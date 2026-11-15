# SoundTracker Cruncher V1.0 (OTT-Design, 1990): status

Status: **not established.** The tool could not be run, so no packed-file format can
be specified yet.

## Method (attempted)

Clean-room, black-box only, using the same procedure as `docs/specs/modcrusher/SPEC.md`:
the plan was to run the tool in an emulator and analyse input/output triples. No 68k
code of the program was read or disassembled. `strings` gives nothing readable: the
executable is itself crunched and has no plain usage text.

## What was tried

* `vamos` (amitools) with default settings, `-m 8192`, `-C 68020/68040` (refused: "too
  much RAM with hw access"), and every `-H` hardware-access mode (`emu`, `ignore`,
  `disable`, `abort`). On start, the program's unpacking stub writes the colour
  registers (`$DFF1A2`...) and then calls `exec FreeMem` on a block vamos does not
  recognise. vamos aborts with an internal FreeMem error (PC $162E, or $2372 with
  `-H disable/abort`). The program never reaches its own code, so it is not a
  configuration problem that more memory would fix.
* The program is a Workbench/Intuition GUI tool (per its distribution), so even under
  a full emulator it needs interactive GUI input to load and save files.

## Open / next step

* Run it under FS-UAE (installed: `/usr/bin/fs-uae`) with the built-in AROS ROM, no
  Kickstart, and a shared host directory as a drive. Drive the GUI by scripted input
  or by hand to produce (input, packed) pairs. If the package has a depacker or a
  player replay routine, use it to get unpack output. No test vectors exist yet.
