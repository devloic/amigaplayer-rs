# Logs and reproduction

Logs are in `logs/`, named `thm.<Module>.sub<N>.ref.log`. They were produced
by the reference 68k player run through the host with DELI_WRITE_LOG
(player selected as the Eagleplayer "Thomas Hermann", dummy audio driver,
wav output discarded, subsong chosen with `--subsong N`, a limit of
120 s of emulated time; shorter logs ended with the song).

Format: one line `ADDRESS VALUE COLOUR-CLOCK` per hardware write (hex
address and value, decimal absolute colour clock). Lines starting with `#`
are comments. Addresses of the form `dff0xx` are custom registers
(Paula audio and DMACON/INTENA/INTREQ/ADKCON); `bfd600`, `bfd700` and
`bfdf00` are the CIA-B timer writes made by sound initialisation. Writes
before the last `bfdf00 0011` line are initialisation (clearing of the
channel registers) and are not replay; everything after it is replay.
Location-register values depend on where the host placed the sample data
and should be compared as "same instrument -> same base plus offset".

| log | module | subsong | notes |
|---|---|---|---|
| thm.Beam.sub0 | Beam (9776 bytes) | 0 | real module, 22 instruments, timer 0x4000 |
| thm.Beam_Unused.sub0/1/2 | Beam, unused variant | 0, 1, 2 | real; subsongs 1 and 2 are short, ending early |
| thm.Blue_Angel_69.sub0..4 | Blue Angel 69 (11792 bytes) | 0..4 | real, 5 subsongs, timer 0x4400 |
| thm.Nightdawn.sub0 | Nightdawn (12176 bytes) | 0 | real, uses the flag 0xFF envelope and portamento rows |
| thm.SynEnv.sub0 | synthetic | 0 | Nightdawn's header/instruments with random patterns; envelope flag 0xFF everywhere |
| thm.SynMode0.sub0 | synthetic | 0 | multi-octave records (mode 0), flags 0/1/0xFF, large transpositions, first position 0xFE |
| thm.SynMix.sub0 | synthetic | 0 | all flags, one disabled voice, speed 2, 12 rows |

Reproduce a log: run the host binary with the reference player on the
module (`thm.<name>` with `smp.<name>` beside it), set DELI_WRITE_LOG to
the output file and use the options above. The synthetic modules are
Nightdawn's module with pattern, pointer and transposition data replaced
by random data; they are in the pipeline's private scratch only, but
every log carries the module's size in its first line.

Comparison: the order, address and value of the writes must be equal; the
colour clock may differ by the figures in the README. `tools/replay-cmp.py`
compares two logs.
