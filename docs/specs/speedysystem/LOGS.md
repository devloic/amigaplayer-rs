# Reference write logs - Speedy System

Produced by running the reference 68k player through the host with
DELI_WRITE_LOG (one line per 16-bit custom-chip write: `ADDRESS VALUE
COLOUR-CLOCK`, address and value in hex, colour clock in decimal from the
start of the run), sound output discarded. The first line of each log,
`# module BASE LENGTH`, gives the guest address at which the module was
loaded (0x45e0 in all logs) and its byte length; sample and loop pointers
written to AUDxLC are absolute guest addresses, so they equal BASE plus a
file offset.

A write of a 32-bit AUDxLC appears as two lines with the same colour clock
(high word at the register, low word at register + 2). The first eight lines
of every log are the initialisation volume writes (SPEC 6.1).

Pointers to the player's own silent sample appear as 0x00689A40 in these logs.
That address is where the reference player happens to keep its zero data; it
is not defined by the module (README).

| Log | Module | Speed | Lines | Last stamp (cc) | Notes |
|-----|--------|-------|-------|-----------------|-------|
| real_trolls.log | the one real module in the corpus (not redistributed) | 2 | 103187 | 722120321 | song end on its tick 10177 (counting the first tick as 1); 78487 merged writes |
| syn0.log | syn0.mod | 2 | 80 | 3499118 | one note, minimal |
| syn_f1.log | syn_f1.mod | 3 | 3673 | 36490492 | randomised rows, all sample shapes |
| syn_f7.log | syn_f7.mod | 3 | 3155 | 36488925 | randomised |
| syn_f13.log | syn_f13.mod | 2 | 2037 | 27624643 | randomised |
| syn_f21.log | syn_f21.mod | 1 | 2193 | 18393270 | randomised |
| syn_f33.log | syn_f33.mod | 3 | 2676 | 36487732 | randomised |
| syn_wrap.log | syn_wrap.mod | 0 | 4562 | 290808972 | all 256 positions, ends by position wrap |

The synthetic modules (`*.mod`, written by a private generator, no player
data) are included so a port can be run against the same input and its output
compared with the log. They carry synthetic 8SVX-style samples of all
shapes (loop, one-shot, repeat-only, short-header, raw) in sample slots
13..19, plus a few slots (0..9) for syn0. The randomised rows cover notes of
all octaves, silent notes (b1 = 0xFF), rejected sample indices, commands
0x50/0x51/other, run codes, and ignored bytes.

Logs end a little after the song-end tick: the host stops one or two ticks
after the end is reported (it polls the end flag once per 4800 audio frames),
so a comparison should stop at the song-end tick or accept the extra
trailing ticks that restart the song from position 0.

## Comparing

Merge each pair of AUDxLC word writes into one 32-bit write if desired, drop
the first eight initialisation lines if the port does not model start-up, and
compare (address, value, colour clock). A model built from SPEC.md reproduces
all logs above with zero differences in address, value and colour clock
(README, validation).
