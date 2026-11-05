# MIDI - Loriciel: reference logs

All logs are register-write logs of the reference 68k player ("MIDI - Loriciel", the
UADE/Eagleplayer Wanted Team adaptation V1.0) run through the host with DELI_WRITE_LOG: one line
per Paula register write, `ADDRESS VALUE COLOUR-CLOCK`, after a `# module LOAD-ADDRESS SIZE` line
(the song file; the bank is loaded elsewhere, at the address the silence writes show).
Captured with `logs/runref.sh MODULE OUT.log [seconds]` (audio to `--wav /dev/null` with
`SDL_AUDIO_DRIVER=dummy`; set `E` to an empty directory, `EAGLEPLAYERS` to the directory holding the
Eagleplayer, `BIN` to the host binary). The module must be given with an absolute path: with a
relative one the host does not find the second file ("could not load additional files"). The offline
render stops shortly after the song end. `tools/replay-cmp.py REF.log PORT.log` compares a port's log
tick by tick.

## Modules

No real module exists in the archive (no `midi.*`, `smpl.*`, `*.mid` or `*.bsp` file name, and no
module header in the archive starts with `MThd` or `BNKS`), so every log is of a synthetic module pair built by
`logs/syn/gen.py` (song `MIDI.mN`, bank `SMPL.mN`; all six share one bank: 4 programs, 5 one-shot
samples, program 1 with three key regions, program 3 with two).

| log | song | writes | exercises |
|---|---|---|---|
| m1 | format 0, division 96, 1 track | 937 | program changes on 4 channels, 8 velocities, note-off by 0x80 and by velocity 0, two tempo changes, song end |
| m2 | format 1, division 120, 3 tracks | 2 973 | tempo track with text/time-signature metas and three tempos, 5-note chords on one channel (voice stealing, failing note-offs), all-voices-busy start on another channel (voice 0), SysEx, controller and pitch-bend skipping, tracks ending at different times |
| m3 | format 0, SMPTE division | 527 | default division, wide note range over key splits (period table from -29 to +29), channel prefix meta |
| m4 | format 0, division 48 | 335 | 0xD0 and a data byte in status position (2-byte skips), 0xA0, 0xF1, unknown meta type, 2-byte VLQ meta and SysEx lengths, note-off without link, a second note-off for a freed voice, stealing of a voice owned by another channel |
| m5 | format 1, division 192, 5 tracks | 1 308 | five channels on four voices, inactive track slots, ends at different times |
| m5_renamed | m5 with the file name `MIDI.longername5` in another directory | 1 308 | the start-up timing terms of the file name and directory lengths (SPEC 7.11) |
| m6 | format 0, division 64 | 381 | tempo 120 000 (latch 5382), tempo 2 000 000 (latch overflow, 60640), deltas 1..13 (not multiples of 4), long deltas |

## What they confirm

- A private model written from SPEC.md reproduces every line of all 7 logs (7 769 writes: address,
  value, order and colour clock), including the eight start-up volume writes, the reset writes of the
  start call, the clock of the first tick, every song-end tick and the ticks after the restart that
  the host renders.
- Execution traces of these runs cover every path of the play routine and of the start-up calls except the host-muted voice
  case (volume 0 from a voice switched off by the host) and the lower-case / `.mid` name spellings of
  the second-file load, the failure exits of loading and a region search that runs past the last region.
- Tick spacing equals `(L + 1) * 5` cc in every log (74 585, 111 880, 44 755 for m1's three tempos;
  37 295 for m3 and m5; 26 920 and 303 205 for m6's fast and overflowed tempos), changing one tick
  after the tick with the tempo event (SPEC 6.2).

## Typical times

Note-on on voice 0 (m1, first note): `LC` and `LEN` 16 cc apart, `PER` 86 after `LEN`, `VOL` 189
after `PER`, `DMACON` 102 after `VOL`. On voice 3 (m2): 16, 120, 243, 136. The write after a
`DMACON` of a note-on or note-off follows the beam wait (SPEC 7.3): 2 121 to 3 529 cc in these logs
(the upper values include the event processing that follows the wait).
Tail writes: `LC`, then `LEN` 8 cc later; the next owned voice's `LC` 43 or 44 cc after the previous
one's when the voices between are owned too.
