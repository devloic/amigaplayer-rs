# Scott Johnston: reference logs

All logs are register-write logs (DELI_WRITE_LOG: one line `ADDRESS VALUE COLOUR-CLOCK` per Paula
write, absolute colour clock from reset; first line `# module BASE SIZE`) of the reference 68k player
(the UADE players directory's `ScottJohnston`, selected with `--player 'Scott Johnston'`) run through the
host, 20 s each, `--wav` to a throw-away file with `SDL_AUDIO_DRIVER=dummy`; nothing was played through
speakers. `logs/runref.sh MODULE OUT.log [seconds]` reproduces them (it needs the variables REF_BIN,
UADE_PLAYERS and EMPTY, see its header). The reference never stops by itself (no song end), so every
run lasts the full 20 s.

## Modules

There is no real module of this format in the corpus. The five modules in `modules/` were built from
the format description (SPEC 1, 2), each in its own folder with an `Instruments/` folder holding the
8SVX files its slots name (synthetic waveforms, 8363 Hz headers, none of the player's bytes). Their
instrument files have `VHDR` at offset 12 and `BODY` at offset 40.

| log | module | timer | what it exercises |
|---|---|---|---|
| sj1-basic.ref.log | modules/sj1-basic/SJ.sj1-basic | 14187 | notes on all voices, 12 instruments with and without loops, volume commands, song loop (0xDE) |
| sj2-effects.ref.log | modules/sj2-effects/SJ.sj2-effects | 11000 | fades up and down (to their limits), hold/release, row delay, loop start/end, DMA off (0x43) on voices 0, 1 and 3, filter off/on, sync flags, a zero slot (repeat of the previous file) |
| sj3-defaulttimer.ref.log | modules/sj3-defaulttimer/SJ.sj3-defaulttimer | default 0xFFFF (timer byte 0xFF) | voice mask 0x05 (voices 1 and 3 stay on the built-in song), "stop here" rows (0xDD) on two voices, trailing zero slots |
| sj4-dense.ref.log | modules/sj4-dense/SJ.sj4-dense | 7000 | 25 rows with notes and commands on every row, fades of every speed, loop of 3 |
| sj5-paths.ref.log | modules/sj5-paths/SJ.sj5-paths | 9000 | hold on voice 0 (changes the first write of the tick), DMA off on voice 2, no-op commands (0x44, 0x89, 0xDF, 0xE4, 0xF9, 0xFD, 0xFE, 0xA6), delay 1, loop count 1, a repeat-only sample (oneShot 0, odd file size), clear sync flags |

## Run facts

| log | player init call (cc) | first DMACON write | song start call ends | first interrupt call | interrupt calls | writes |
|---|---|---|---|---|---|---|
| sj1-basic | 2 410 | 5 742 | 39 682 | 110 622 | 999 | 14 421 |
| sj2-effects | 1 896 | 5 123 | 39 001 | 94 006 | 1 289 | 18 376 |
| sj3-defaulttimer | 1 254 | 4 348 | 38 069 | 365 749 | 216 | 2 877 |
| sj4-dense | 1 639 | 4 812 | 38 774 | 73 779 | 2 026 | 32 801 |
| sj5-paths | 1 382 | 4 503 | 38 320 | 83 325 | 1 576 | 22 860 |

The interrupt call clocks are not in the write logs (they come from the reference's call trace); the
first write of a tick whose voice 0 is not on hold is 221 cc after the call when voice 0 refreshes the
empty word and 227 cc when it refreshes a loop. Interrupt `n` is called at
`first + n * 5 * (timer + 1)`.

Addresses in the logs (the same in all five runs): the module is loaded at $45E0; the instrument files
are loaded one after the other from $4DE0 in slot order, each in its own host allocation of
`file size + 16` bytes rounded up to a multiple of 8 (the next file starts right after); a sample's AUDxLC is its file's address plus the offset of the BODY data. The
player's empty word (SPEC 5.1) is at $A0CF84. tools/replay-cmp.py maps module addresses to module offsets
and requires every other address to differ by one constant between the two runs: a port must place its
empty word and its instrument data with the same distances as above (or at the same addresses).

## What was checked

A private model that follows SPEC.md was run against all five logs, tick by tick: every write's address,
value, order and absolute colour clock (the interrupt call clocks of the table above as the only input
besides the module) equals the reference: 6 111 ticks after the init group, 91 335 writes, 0 differences.
The init group (SPEC 4.1) was compared by address and value; its timing is described in SPEC 9.5 from
the logs. The cost constants were also checked by perturbation: changing a helper's after-cost by 2
cycles breaks every tick.

## Gap tables

`logs/gaps_tsv.py LOG FIRST_CALL PERIOD` prints, per write: tick, index in the tick, address, value,
cc from the interrupt call, cc from the previous write. `logs/sj2-effects.gaps.tsv` (first call 94006,
period 55005) and `logs/sj4-dense.gaps.tsv` (first call 73779, period 35005) are included.

Typical gaps (cc), all logs:

| step | gap |
|---|---|
| refresh LC to LEN, loop / empty word | 103 / 99 |
| refresh LEN to next voice's LC: loop / empty / previous voice fading but not stepping | 178 / 172 / 197 |
| note start LC to LEN to VOL to PER to DMACON | 103, 91, 91, 100 |
| DMACON on to the next note start's LC | 170 cc after a raster line start (227 cc lines), 210 with an idle voice between |
| note DMACON off to the next voice's | 243 (adjacent), 279 (one voice between) |
| fade VOL to the next voice's refresh LC | 161 |
