# Howie Davies - timing and reference logs

Logs in `logs/` come from the reference 68k player run through the host with
DELI_WRITE_LOG. File names: `<module>.<subsong>.log` with module one of
`blazingthunder` (subsong 0), `futurebike` (0-2), `yogi` (0-7). Columns:
register address (hex, without 0x), value (hex), colour clock since the start
of the player's initialisation. A long write to a voice pointer appears as two
lines (high word at the register, low word at register + 2) with the same
colour clock. CIA writes (Blazing Thunder init: address bfe001, value 0002)
appear as plain lines too.

## What each file holds

| file | content |
|---|---|
| blazingthunder.0 | first 20 s (71 M colour clocks) |
| futurebike.0 | first 20 s |
| futurebike.1, futurebike.2 | complete: until the host stopped (a few ticks after the song end; the tail is DMACON 0x8000 once per tick) |
| yogi.0 .. yogi.6 | first 12 s |
| yogi.7 | complete: until the host stopped right after the song end |

Runs for subsong n > 0 start with the host stepping through the subsongs
(SPEC 8.2): the log begins with the init blocks of subsongs 0..n-1 and their
EndSound blocks (writes DMACON 000f, then AUDxVOL 0 for the four voices).
Validation used the full 120 s runs of the same configurations (about
6000 ticks); those longer files are not shipped.

## Values that depend on the host and must be normalised before comparing

- Voice pointer writes: the reference loads each module at a fixed address
  (Blazing Thunder 0x1B240, Future Bike 0x16860, Yogi's 0x1CFE8), so a
  pointer is load address + hunk offset of the sample.
- The silent address in the reference is 0x00798A58 (lines "0079" / "8a58").
  Any other silent chip address is equivalent.

## Global timing

- Interrupt period: 70940 colour clocks. The first interrupt is 70940 colour
  clocks after the end of the last InitSound call.
- First InitSound call clock: 957 + half of the start-up scan cost
  (SPEC 11.1): Blazing Thunder 38646, Future Bike 38465, Yogi's 41083.
  InitSound call length: 1741 cc (Blazing Thunder), 1477 (Future Bike),
  1513 (Yogi's).
- Cost rule: the module's execution cost in 68000 cycles from the start of a
  call, halved (rounded down), is the colour-clock offset of each write
  (SPEC 10.1). With the constants in SPEC 10 all logs are reproduced to the
  colour clock.

## Timing landmarks seen in the logs

- First write of an interrupt (DMACON 0x800F or 0x8000|mask) = interrupt start
  + 148 cc (variants A and C) or + 150 cc (variant B); that is (256 wrapper +
  40 or 44 cycles) / 2.
- The pointer writes of the DMA-on routine follow after the raster wait:
  1816-2043 cc after the DMACON write for the A/C helper (nine line changes,
  depends on the beam phase; seen 1983 for Yogi's, 2145 for Blazing Thunder
  where the phase differs), 1850 for Future Bike (eight lines).
  Then pointer, length 10 cc apart (A/C) or 8 cc apart (B); next voice 83 cc
  (A/C) or 71 cc (B) after the previous voice's pointer.
- Note start (offsets from the DMACON bit write): pointer +62, period +72,
  length +82; the volume write comes 301-345 cc after the DMACON write
  (A: 301/302, 325/326, 344 for voices 0, 1, 2; C: 309/315, 333/339 for voices
  0, 1; B: 285, 309, 328, 345/346 for voices 0..3). The two values per voice
  come from the parity of the cycle count.
- The tick-to-tick period is exactly 70940 cc; ticks without a step contain
  only the DMA-on writes and the envelope volume writes.

## Song end

The song-end notification costs an extra 83 cycles (Blazing Thunder / Yogi's)
or 84 cycles (Future Bike) on the command that raises it. In the logs this
shows only as a 41.5 / 42 cc shift of the following writes of the same
interrupt, which is why those two numbers are given separately (Yogi's subsong
7 at colour clock 314254873; Future Bike subsong 1 at 11607878, subsong 2 at
27361194).
