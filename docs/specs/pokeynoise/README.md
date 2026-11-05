# Pokeynoise: unknowns, caveats, implementation notes

What this spec is: the behaviour of the Eagleplayer "POKEYNoise V0.25alpha" for Atari 8-bit SAP files
(types B, M, C) and Atari binary-load files: file handling, its own 6502 interpreter (with its
deviations), and its POKEY-to-Paula conversion. 
## What to take from where

- From this spec: everything that defines the output. Loader and header parsing (SPEC 1), host
  calls (2), the interpreter's rules (3), the conversion (4), the Paula sequence (5), timing (6) and
  the waveform bank (7, `data/`).
- From ASAP (GPL-2-or-later), optionally: the instruction semantics of the 151 documented 6502
  opcodes. ASAP's CPU core differs from this player in the stack (wrapping, no end-of-call
  detection), BRK, decimal mode, undocumented opcodes, the zero flag after ADC/SBC, PHP's B bit, the
  hardware read and write hooks and its cycle budget; all of those must follow SPEC 3. A small
  dedicated interpreter may be simpler than adapting it.
- Not needed from ASAP: its POKEY emulation and sound generation. The player never synthesises POKEY
  sound; it only reads the register bytes back from memory after each call and picks a waveform and
  period. RANDOM is the player's own generator, not a POKEY polynomial.

## Data

`data/pokeynoise_waves.bin` (48 908 bytes) is the player's waveform bank, byte for byte. It cannot
be generated from a rule given here (the waveforms are filtered renderings). The port embeds it on the project owner's decision; it is player data, not module data. Only 33 waveforms are reachable (SPEC 4.6);
the rest can be dropped.

## Unknowns and caveats

- Raw binary-load files depend on an icon (`.info`) next to the module, read through icon.library;
  untested here. Without one the file is not detected.
- VCOUNT (`$D40B`) and the start time of a tick's writes depend on how long the emulated 6502 takes,
  which only the original interpreter's cost per instruction defines; only rough figures are given
  (about 31 cc per NOP, 3 500 to 4 000 cc for the conversion). Modules that busy-wait on VCOUNT play
  correctly but their timing will not be register-exact.
- The wait for the audio interrupt after switching a playing voice off belongs to the host's Paula;
  a port must wait on the same flag to get the same delays (up to about 65 000 cc, see SPEC 6.5).
- JOY1DAT enters the random generator; it is 0 in this host. A host returning mouse/joystick data
  changes the random silencing and RANDOM values.
- Subsong changes keep the emulated memory, the history bytes and the generator state, so a song
  started after another may not sound like a fresh start. The host's own sequence when a non-default
  subsong is requested (it stops the default song first, then steps songs) is a host property.
- No song end: modules play forever. No title or author is reported.
- An endless 6502 loop (for example waiting for the real-time clock inside the play routine) hangs
  the reference; a port may want a safety limit, which the reference does not have.
- Program counter overflow past `$FFFF` and the stack pointer word after `TSX` with an exhausted
  stack are undefined in the reference and not specified.
