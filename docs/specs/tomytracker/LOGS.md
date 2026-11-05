# Tomy Tracker: reference logs

All logs are register-write logs of the reference 68k player ("Tomy Tracker", the UADE/Eagleplayer
Wanted Team adaptation V1.1) run through the host with DELI_WRITE_LOG: one line per Paula (and CIA)
register write, `ADDRESS VALUE COLOUR-CLOCK`, after a `# module LOAD-ADDRESS SIZE` line.
Captured with `logs/runref.sh MODULE OUT.log [seconds]` (audio to `--wav /dev/null` with
`SDL_AUDIO_DRIVER=dummy`). The offline render stops at the song end, sometimes 1 to 4 ticks later.
`tools/replay-cmp.py REF.log PORT.log` compares a port's log tick by tick.

## Modules

No real Tomy Tracker module exists in the archive, so every log is of a synthetic module (built by
`logs/syn/mksyn.py`; `logs/syn/README.txt` says what each one exercises).

| log | writes | reaches song end |
|---|---|---|
| syn1 | 2 802 | yes |
| syn2 | 1 490 | yes |
| syn3 | 1 620 | yes |
| syn4 | 999 | yes |
| rnd1 | 5 809 | yes (+2 ticks) |
| rnd2 | 8 405 | yes (+4 ticks) |
| rnd3 | 8 551 | yes (+3 ticks) |
| rnd4 | 4 194 | yes (+4 ticks) |
| rnd5 | 5 373 | yes |
| rnd6 | 6 660 | yes |
| rnd7 | 10 616 | yes (+2 ticks) |
| rnd8 | 2 704 | yes |

## What they confirm

- A private model written from SPEC.md reproduces every line of all 12 logs (58 423 writes:
  address, value, order and colour clock), including the eight start-up volume writes, the clock
  of the first tick, the song-end ticks (both parities of the cycle count at the notification) and
  the ticks after the restart.
- Execution traces of these runs cover every path of the reference's play routine (all commands,
  both porta directions reaching and not reaching the target, every clamp, both vibrato halves,
  all three arpeggio phases).
- Tick spacing is 70 940 cc; the first tick is 70 940 cc after the start call ends (75 719 for
  syn1, whose start-up calls end at 4 779).

## Typical times (cc from the tick call), syn1

Row tick, channel 0 starts a sample, channels 1 to 3 empty:
LC0 373, LEN0 379, PER0 415, VOL0 516, PER1 635, VOL1 747, PER2 866, VOL2 990, PER3 1108,
VOL3 1244, DMACON off 1279, DMACON on 3550, then LC/LEN of channels 0..3 at 5595/5601, 5613/5621,
5633/5641, 5653/5661.

Row tick, all channels empty: PER0 268, VOL0 369, PER1 487, VOL1 600, PER2 718, VOL2 842,
PER3 961, VOL3 1096, DMACON 1131 and 3315, LC/LEN from 5347.

The DMACON on time and the loop writes follow the beam (SPEC 7.3): DMACON on comes 2146 to 2491 cc
after DMACON off in these logs, the first loop LC 2032 to 2045 cc after DMACON on.
