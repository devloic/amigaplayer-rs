# Reference write logs - Speedy A1 System

Produced by running the reference 68k player through the host with DELI_WRITE_LOG (one line per 16-bit
custom-chip write: `ADDRESS VALUE COLOUR-CLOCK`, address and value in hex, colour clock in decimal from
the start of the run), sound output discarded (dummy audio, nothing played). The first line of each log,
`# module BASE LENGTH`, gives the guest address at which the module was loaded (0x45e0 in all logs) and
its byte length; sample and loop pointers written to AUDxLC are absolute guest addresses, so they equal
BASE plus a file offset (table base TB plus a table-2 entry, SPEC 1.2).

A write of a 32-bit AUDxLC appears as two lines with the same colour clock (high word at the register,
low word at register + 2). The first eight lines of every log are the start-up volume writes (SPEC 8).
Pointers to the player's own silent sample appear as 0x0068ACB0 (high word 0x0068, low word 0xACB0). That
address is where the reference player happens to keep its zero data; it is not defined by the module
(README).

## Files

| log | module | module bytes | speed | start / end | write lines | last stamp (cc) | notes |
|---|---|---|---|---|---|---|---|
| a1_endorder.log | a1_endorder.mod | 10734 | 3 | 0 / 2 | 899 | 4924864 | song end by an order entry 0xFFFF |
| a1_endpos.log | a1_endpos.mod | 9888 | 0 | 0 / 6 | 2721 | 7048235 | song end by the position test; effects 0x50/0x51/0x52 |
| a1_endrow.log | a1_endrow.mod | 9498 | 0 | 0 / 6 | 1022 | 2794678 | song end by row byte 0xFF |
| a1_env5.log | a1_env5.mod | 10078 | 5 | 1 / 7 | 5054 | 34353371 | long run: attack/release incl. the release-value-1 case, idle transitions, start position 1 |
| a1_long9.log | a1_long9.mod | 9806 | 9 | 0 / 3 | 5267 | 33643555 | speed 9, many envelope steps |
| a1_mixa.log | a1_mixa.mod | 14648 | 3 | 0 / 4 | 3569 | 18037001 | randomised rows, all kinds |
| a1_mixb.log | a1_mixb.mod | 8892 | 2 | 0 / 4 | 3115 | 13780483 | randomised rows, all kinds |
| a1_spd0.log | a1_spd0.mod | 12610 | 0 | 0 / 4 | 1234 | 3151610 | speed 0 (every tick is a full tick) |
| a1_spd3.log | a1_spd3.mod | 10384 | 3 | 0 / 3 | 2228 | 13437789 | speed 3 |
| a1_start2.log | a1_start2.mod | 9502 | 1 | 2 / 4 | 629 | 2429912 | start position 2, end 4 |
| a1_syn0.log | a1_syn0.mod | 6224 | 2 | 0 / 1 | 687 | 3503301 | hand-made minimal module (four notes) |
| a1_weirda.log | a1_weirda.mod | 19600 | 2 | 0 / 4 | 3778 | 13781054 | odd sample-table values (lengths below 0x68, odd loop values) |
| a1_weirdb.log | a1_weirdb.mod | 22162 | 2 | 0 / 4 | 3539 | 13780982 | odd sample-table values |
| a1_real.log.xz | the one real module found (not redistributed) | 132892 | 2 | 0 / 0 | 472896 | 1641570449 | xz-compressed; 462.7 s song, ends by an order entry 0xFFFF at tick 23137; the log continues 17 s into the restarted song |

The synthetic modules (`*.mod`, written by a private generator, no player data) carry random 8SVX-style
objects (their bytes are never read by the player) and are included so a port can be run against the same
input and its output compared with the log. Their rows are randomised over: notes of all octaves,
notes with period 0, sample indices at and above 0x20, the commands 0x50, 0x51, 0x52 and inert bytes,
all three envelope kinds (b1 = 0, high nibble 0, other), looped and unlooped samples, empty sample slots,
and the three kinds of song end (row byte 0xFF, order entry 0xFFFF, position test). The `weird`
modules have sample-table values that are not consistent with an IFF layout, to check the arithmetic.

Logs end shortly after the song-end tick: the host stops one or two ticks after the end is reported (it
polls the end flag once per 4800 audio frames), so a comparison should stop at the song-end tick or accept
the trailing ticks that restart the song at the start position.

The real module is the demozoo file `digital voyage.sas`; the corpus query for the prefix `sas.` finds
nothing because this file uses the extension form of the name.

## Reproduce a reference log (silent)

Run the host with the Eagleplayer named "Speedy A1 System", dummy audio, `--wav /dev/null` and
DELI_WRITE_LOG set; no speakers are used. Nothing else is needed (the player has no external files).

## Comparing

Merge each pair of AUDxLC word writes into one 32-bit write if desired, drop the first eight lines if the
port does not model start-up, and compare (address, value, colour clock). A model built from SPEC.md
reproduces every log above with zero differences in address, value and colour clock (README, validation).
