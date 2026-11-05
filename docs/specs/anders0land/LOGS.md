# Reference logs: Anders 0land

Logs are in `logs/<module>.<subsong>.log`; the matching synthetic module is `modules/hot.<module>`. All 14 are
synthetic (no real module was found), produced by the reference 68k player run through the host with DELI_WRITE_LOG.

## Format
- First line: `# module <load address in hex> <module size in bytes>` (load address 0045e0 in every log).
- Then one line per register write: `<chip register address> <16-bit value> <colour clock>`.
- 32-bit pointer registers (AUDxLC) appear as high half then low half, both at the same colour clock.
- Pointer values are absolute. To compare against a module-relative value: pointer = load address + size of
  chunk 1 + size of chunk 2 + 8 + table value (the table values in the song data are relative to the start of
  the sample chunk's payload). Normalise by subtracting the load address from the first line.
- A log for subsong n > 0 also contains the start-up blocks of subsongs 0..n-1 (init, then the song-end
  routine ran immediately); only the part after the last CIA line (`bfe001 0002`) is the played subsong.

## Runs
| run | subsong | ticks | writes | initial speed | voice mask | notes |
|---|---|---|---|---|---|---|
| mod01 | 0 | 319 | 3489 | 2 | F | ends by song end |
| mod02 | 2 | 159 | 1749 | 10 | F | ends by song end |
| mod03 | 1 | 404 | 4347 | 2 | F | stop marker |
| mod04 | 1 | 1064 | 11904 | 5 | F | ends by song end |
| mod05 | 0 | 999 | 11598 | 1 | F | cut at 999 ticks |
| mod06 | 1 | 999 | 7932 | 8 | 7 | three voices, cut |
| mod07 | 1 | 999 | 8583 | 1 | E | three voices, cut |
| mod08 | 2 | 1144 | 12270 | 8 | F | ends by song end |
| mod09 | 0 | 219 | 2487 | 3 | F | ends by song end |
| mod10 | 2 | 264 | 2709 | 10 | F | ends by song end |
| mod11 | 0 | 999 | 9357 | 7 | song F, start 2 | cut |
| mod12 | 1 | 999 | 6360 | 3 | A | two voices, cut |
| notes | 0 | 399 | 1794 | 2 | 1 | note index sweep 0..69 |
| speed0 | 0 | 1539 | 13923 | 0 | F | tempo 0 (256-tick rows), ends by song end |

Together: 10 506 ticks, 98 502 writes. "Ticks" counts the tick boundaries in the log (the frame period,
70940 cc). Runs cut at 999 ticks were stopped by the harness limit; the others end a few ticks (2 to 5) after the
song-end notification, when the host stops.

## Timing landmarks
- Let T be the colour clock of the CIA line of the played subsong. Start sequence: DMACON at T+10, the four
  volume writes at T+20..T+50.
- Tick k (counting from 0) has its first output write at roughly T_init + 74267 + 70940*k, where T_init is the
  clock of the last write made by the init routine; SPEC.md gives the per-write offsets inside a tick.
- Before the first subsong's CIA line the host writes eight zero volumes at -1853, -1710, -1558, -1397, -1137,
  -994, -842, -681 cc relative to it. After each song-end notification of a stepped-over subsong another eight
  follow at +3648, +3791, +3943, +4104, +4364, +4507, +4659, +4820 relative to that subsong's init.
- Stepped-over subsongs repeat every 5501 cc; the song-end block sits 3391 cc after its init.
- The song-end notification advances time by its measured count converted to cc (floored) plus 8 cc.
