# Major Tom: reference logs

All logs are register-write logs (lines `ADDRESS VALUE COLOUR-CLOCK`, first line `# module BASE SIZE`)
of the reference 68k player (UADE "MajorTom", shown by the host as "Major Tom's Player 2.01") run through
the host with DELI_WRITE_LOG, `--player "Major Tom's Player 2.01"`, the UADE player directory as
EAGLEPLAYERS, empty DeliPlayer/DeliGenie directories, audio to a WAV file with SDL_AUDIO_DRIVER=dummy.
Runs that reach the song end stop there (the host stops 1-5 ticks after the notification); the others
stop at the `--seconds` limit.

Load addresses in these runs (host layout, not meaningful for a port): module 0x0045E0; the player's
noise block 0x0076C1E0; its 4-byte zero area 0x0076CD98 (noise block + 3000). Noise and zero-area
addresses are "other" addresses for tools/replay-cmp.py and differ from a port's by one constant if the
port keeps the zero area right after the noise block.

## Real modules (`logs/real/*.log`, full songs)

The nine known files (all from demozoo, all with the standard layout of SPEC 2.1). The modules are not
included; the md5 identifies them.

| log | md5 | size | L | R | song end at tick | what it shows |
|---|---|---|---|---|---|---|
| b-manipulation | 9044864099136db4ec60ac013002ec7c | 38464 | 18 | 0 | 3457 | speed effects, porta, noise, sequence jumps |
| king_of_games | 219f71cfc914c0414c6234992b3960b7 | 43584 | 26 | 5 | 4993 | period-table overrun indices 89, 93, 96, 103 (SPEC 4.8); restart 5 |
| memorybank_overflow | 70db5fa1312eb8e15988a7c1d0804bc9 | 37440 | 30 | 0 | 4801 | alternating speeds 2/3 every row (F effects), transposes |
| pseudofever | 2076318b908b62e4599f53734f006a86 | 39488 | 24 | 0 | 4609 | slides, porta |
| r.e.a.l | 86e1ff41fe04ef140fa1adadb2331cf9 | 33344 | 12 | 0 | 2305 | the SPEC 5.4 example |
| slashback | 848300786c0d5465dc6a40b787e8e748 | 45632 | 8 | 0 | 1537 | transpose 1 |
| spacedust | cc205f53b13d1f9f2e035f2275ac7f1d | 36416 | 22 | 2 | 4225 | many noise instruments |
| the_holy_noise | 154e8cb5bdd893c32165ea9b20477b62 | 33344 | 21 | 1 | 4033 | slides both ways, index 91 |
| thought_kontrol | f2c9afedd15081c4ab25cfa2f3bfb87b | 42560 | 18 | 0 | 3457 | waveform jumps whose read uses the sequence's own byte 0 |

Every real module uses waveform-sequence jumps. Their reads (SPEC 4.7) land either inside the module
(jump long 0: the sequence's own speed byte) or 1.0-2.4 MB beyond it, where the reference reads 0.

## Synthetic modules (`modules/arp.*`, logs `logs/syn/arp.*.log`)

No other ARP. files exist in the corpus index; these were built from the format description to reach what
the real files do not.

| module | seconds | what it exercises |
|---|---|---|
| dbreak | 30 | effect D without a note, with a note+instrument (one tick late), two voices breaking in one row (position + 2), break on the last row, break at the last position (wrap to 0, no song end; never ends) |
| speed | 120 | effect F on voices 1..3, F with a note, two F in one row (later voice wins), speeds 1..6 |
| porta | 120 | porta up/down, porta continuing after the effect ends, notes without instrument, slides with large parameters, transposes 2 and 0xFE, restart 1 |
| seqs | 120 | jumps in all four banks, jump past position 63, waveform jump reads inside the module (offset 0 and 40), noise on/off, waveform value 0, volumes 0x80/0xFF, pitch values >= 0x10, speed-0 sequences, position clamps 63 -> 62/60 |
| idle | 120 | three voices that never get a note: idle period index 180 written every 256 ticks |
| r00 .. r19 | 20 | random modules: random notes/effects (incl. D and F), random sequences with jumps, waveform jump reads both inside the module and wrapping at 24 bits (r08, r15) |

## What they confirm

A model written from SPEC.md reproduces every write (address, value, order, colour clock) of all 34 logs
(9 real, 25 synthetic; 812 291 log lines), including the start-up writes, the first-tick clock and the
song-end tick. tools/replay-cmp.py on model logs against the reference: no difference.

## Not covered by logs

- Host voice volume other than 64 (SPEC 4.12).
- Period-table indices other than 0..103 and 180.
- Pattern numbers >= 32 (negative offsets, SPEC 2.4), note bytes below 0x10 other than 0, speed 0.
- Waveform-jump reads that land outside the module at a place where the reference host's memory is not
  zero (all observed ones read 0).
