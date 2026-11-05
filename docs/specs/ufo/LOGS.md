# UFO reference write logs

`logs/NAME.ref.log` is the Paula/custom-register write log of the reference 68k player run through the host
with DELI_WRITE_LOG, for the synthetic module `mods/NAME.mus` with its bank `mods/NAME.BANK`.

Line format: `ADDRESS VALUE COLOUR-CLOCK` (hex address, hex 16-bit value, decimal clock). The first line is
`# module BASE SIZE` (hex load address of the song, decimal size). The sample bank is loaded at
(BASE + max(SIZE,1024) + 1024) rounded up to a multiple of 8, which resolves the sample pointer writes.
A 32-bit pointer write appears as two word writes at the same clock.

Run recipe: the reference player "UFO" (Eagleplayer set), host binary with `--player UFO --wav /dev/null
--seconds N`, SDL_AUDIO_DRIVER=dummy, DELI_WRITE_LOG=file, DELI_NATIVE=0, empty player/genie directories.
The host stops at the first song-end signal.

Files: 222 logs. t1 (first test), r1..r12, r20..r43, r100..r111 (random songs of different tempo, track
count, notes, bends), h1..h15 (hand-built: running status, call/return, volume scale, tempo changes,
overlapping notes, embedded instruments, restart/end), and edge probes (b_*, e*, ex*: note limits, bend
extremes, timing experiments with a single note). All are synthetic.
Use tools/replay-cmp.py to compare a candidate log with these.
