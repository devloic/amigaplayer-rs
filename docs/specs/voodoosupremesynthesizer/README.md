# Voodoo Supreme Synthesizer: unknowns and caveats

- Corpus: 7 real modules (6 one-subsong modules and one with 4 subsongs). The reference host cannot select
  a subsong (SPEC 1.2), so subsongs 1 to 3 were logged with a copy of the module whose header slot 0
  holds the other subsong's offset; that edit changes only which song table is played.
- Validation: see LOGS.md. Two models match all 13 runs exactly: the author's, and a second one
  written by a separate agent from SPEC.md, LOGS.md and data/ alone, which matched on its first run. It
  raised five unclear wordings, now made explicit in SPEC (0x88 write-time origin, the remainder test of
  0x7E in both directions, the period used by the pitch-corrected sample step, the interrupt handler's
  bit-5 test, the copy cost). This is not a check by a different person on a different host.
- Paths never reached by any run, real or synthetic: undefined command bytes (0x80, 0x8E..0xFE: the
  reference jumps to an arbitrary address); portamento with `len = 0` or equal notes (divide by zero);
  note indices of 96 or more (read past the period table, SPEC 1.4); subroutine nesting deeper than 20
  stack entries; a loop count of 0; a pitch-corrected sample step with a reference period of 0 (divide
  by zero); envelope or sequence loops that point at a loop marker forever (the reference hangs).
- Unset pointers read machine memory from address 0 (SPEC 5). Two real modules depend on this; the
  bytes given are those of the reference host, another host (or real hardware) has other bytes there.
- Mode D plays the last block of a sample past its end (module bytes that follow); a port must read the
  same memory to match.
- The tick-call times (frame start, delayed by a running audio interrupt) and the audio interrupt times
  are host and Paula behaviour. The spec gives the costs inside each call; the interrupt sequence of a port
  equals the reference only if its Paula raises the interrupts at the same times.
- Host speed controls (the player can repeat the tick up to 6 times per frame) and the host's volume and
  balance (volume scale, SPEC 5) were left at their defaults.
- The format has no title, author or duration; the song end (four jumps in one tick) is the only loop
  point. The reference host stops at it.
- data/period.bin (96 note periods) and data/ratio.bin (13 semitone ratio pairs) are tables of the player,
  not of the modules; the lead decides on embedding them.
- Working notes, scripts and the validation model are in a private folder next to this file.
