# MCMD: reference logs

## How they were made

The reference 68k player (Eagleplayer "MCMD" from the UADE player set) run through the host with
DELI_WRITE_LOG, the WAV sink and the dummy SDL audio driver (silent), empty DeliTracker/genie dirs,
`--player 'MCMD' --seconds 30` (10 s for some). A run with the interpreter instead of the native
recompiler (DELI_NATIVE=0) gives byte-identical write logs; it was used for call/beam-read traces.

Log lines: `ADDRESS VALUE CLOCK` (hex register address, hex value, decimal colour clock). The first line
`# module BASE SIZE` gives the load address of the module (AUDxLC values inside the module are
BASE + offset). The CIA write `bfe001 0002` at init is in the logs too.

## Modules (all synthetic)

No real module of this format exists in the archive index (no file with the "MCMD" magic, none with the
`mcmd_org.` prefix). All 22 modules in `modules/` were built for this spec: 6 by hand, 16 random ones
(generator rules: valid sample/instrument/sequence numbers, note indices kept inside the period table,
sequences end in a hold or loop back to a value, every command 0xE0..0xE8 of both sequences used).

| module | content |
|---|---|
| basic | 2 positions, arpeggio loop, volume decay, trigger notes on all voices, speed 6 |
| vib | vibrato with delay, 0xE5 same/new sample, legato note, break, voice percentage 0xF4/0xF0, volume wait |
| porta | portamento up/down (speed from previous row), effect sequence from previous row, stop command 0x82 (then 1 400+ ticks in stop state) |
| dense | speed 2, every voice retriggers every row, effect and volume waits |
| end | percentage 0xF8, restart (song end at tick 73) |
| e7 | 0xE7 sequence switch, 0xE6 cursor lag with 0xE3, 0xE8 0, volume 0xE8 0/3, volume speed 0 |
| r00..r15 | random modules: all of the above mixed, pattern sizes 16/32/64, 1-6 ticks per row |

## Validation of the spec

A model written from SPEC.md (values and the cycle/clock model of section 11) compared to these logs,
write by write (register, value, colour clock):

| module | writes before the song-end tick | ticks | mismatches |
|---|---|---|---|
| basic | 1 874 | 192 | 0 |
| dense | 1 590 | 64 | 0 |
| e7 | 937 | 96 | 0 |
| end | 796 | 72 | 0 |
| porta | 7 837 | 1 499 (no song end) | 0 |
| vib | 1 348 | 145 | 0 |
| r00..r15 | 26 660 | 2 309 | 0 |
| total | 41 042 | | 0 |

A second, independent model written by a separate agent from SPEC.md alone (no access to the player,
the notes or the logs) gives the same result: 41 042 writes, 0 mismatches. It needed two wording fixes
that are now in SPEC.md (5.1 step 5, 11.3).

The song-end tick itself (8-29 writes per module) has the exact timing for every write, but 1-4 values
differ: they come from the host-memory pattern of the wrap quirk (SPEC 10.3). The reference log then has
one more tick, after which the host stops.

Not covered by any log: the voice mask of the host (masked voice volume 0 and its +4 cycle cost), host
volume/balance other than full, effect/volume commands 0xE9..0xFF (unsupported), section lengths of
32 768 bytes or more, an odd pattern size, period indices 73..127 other than through the initial note of
voice 0 (which reads 0 there in the reference).
