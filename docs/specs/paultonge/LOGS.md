# Logs and reproduction

All logs are of the reference: the 68k player "Paul Tonge" from the UADE players directory, run through the host
(`amigaplayer-rs` reference build) with DELI_WRITE_LOG, `--wav` to a file or `/dev/null`, `SDL_AUDIO_DRIVER=dummy`,
`--seconds 400` (every song ends earlier: the host stops 2 to 3 ticks after the tick that reports the song end;
those ticks are in the logs). Nothing was played through speakers.

## Files in logs/

| file | content |
|---|---|
| `PAT.<song>.ref.log` | write log (`ADDRESS VALUE COLOUR-CLOCK`, first line `# module BASE SIZE`) from the start to where the host stopped, for TUNE1..TUNE8 and title |
| `PAT.<song>.calls.tsv` | one line per call of the play routine (`tick`) and of the audio interrupt handler (`irq`): call time and return time in colour clocks, from the host's call trace (no addresses) |

| song | tick of the song-end report | song end by | sample bank | bank base in the reference |
|---|---|---|---|---|
| PAT.TUNE1 | 1601 | position lists | SMP.set | 0x4DE0 |
| PAT.TUNE2 | 1281 | position lists (3 voices) | SMP.set | 0x4DE0 |
| PAT.TUNE3 | 961 | position lists | SMP.set | 0x4DE0 |
| PAT.TUNE4 | 1385 | position lists | SMP.set | 0x4DE0 |
| PAT.TUNE5 | 1537 | command 0x8F | SMP.set | 0x4DE0 |
| PAT.TUNE6 | 2317 | position lists | SMP.set | 0x4DE0 |
| PAT.TUNE7 | 2881 | position lists (fade command present) | SMP.set | 0x4DE0 |
| PAT.TUNE8 | 1154 (defined up to 864) | command 0x8F | SMP.set | 0x4DF0 |
| PAT.title | 3761 | command 0x8F | SMP.title | 0x51E8 |

The song is always loaded at 0x45E0 (the `# module` line). Sample addresses in AUDxLC writes are absolute (bank base
+ offset); compare modulo the bank base. Ticks are 70 940 cc apart; the first tick's call time depends on the run
(97 900 to 98 346 cc, see the call tables); its first write is always 221 cc after the call. After a stop command
(TUNE5, TUNE8, title) the logs show the DMACON = 0x000F write, then the restart of the song in the next tick (SPEC
5.10) before the host stops.

## Modules (modules/)
`PAT.TUNE1`..`PAT.TUNE8`, `PAT.title`, `SMP.set`, `SMP.title`: one complete game set (Paul Tonge, 1993; credits in
the set name Paul Tonge for sound and music). md5 prefixes: TUNE1 607fa8d7a49c, TUNE2 7795cf2a65a2, TUNE3
19435767e2bf, TUNE4 12178bce42c9, TUNE5 1e35824e46ac, TUNE6 5ef88969b20d, TUNE7 a52948cd3d62, TUNE8 f9c3e244ada1,
title a717edf8c625, SMP.set 9c9ac215b9f5, SMP.title 17900f76e1c1. The driver file of the set (`WantedTeam.bin`, md5
prefix acc16ad08197) is needed by the reference but is not included here. All modules are real; none is synthetic.

## Reproduce a reference log (silent)

Put the set (the `PAT.*`, `SMP.*` files and the driver file `WantedTeam.bin`) in one directory and give the absolute
path of the song file; the player loads the sample bank and the driver from the same directory. The player is
selected by its name `Paul Tonge` (not by the file name).

    E=<empty player directory>
    SDL_AUDIO_DRIVER=dummy RUST_LOG=error DELIPLAYERS=$E DELIGENIES=$E \
    EAGLEPLAYERS=<UADE players directory> DELI_WRITE_LOG=/path/ref.log \
    <reference amigaplayer-rs binary> --player 'Paul Tonge' --wav /dev/null --seconds 400 /ABS/SETDIR/PAT.TUNE1

The call table comes from the same run with the host's call trace enabled (DELI_TRACE); only its call/return lines
are used. A port's log: `--open-replayers` with empty player directories and DELI_WRITE_LOG; compare tick by tick
with tools/replay-cmp.py REF.log PORT.log.

## What was checked

A private value model written from SPEC.md, and a timing model using only the numbers of SPEC section 6, were run
against every log above. For each tick the model gets the call time from the call table (as a port gets it from the
engine); for each audio-interrupt group it gets the call time and the set of channels the reference served (which
depends on the emulated Paula, 5.11).

| check | result |
|---|---|
| register, value, order of every write group (ticks and interrupt groups) | identical for all 9 songs to the end of each log (including the ticks after the song end and the restart after a stop); PAT.TUNE8 up to tick 864 (then undefined, SPEC 7) |
| time of every write (colour clock) | identical: 276 518 writes, 0 differences |
| return time of every tick call and handler call | identical: 18 152 returns, 0 differences (the returns of the song-end tick and of the ticks after it are not counted; their writes are) |

The cost numbers of section 6 are sums of the reference CPU's per-step cycle costs, measured from the host's
per-instruction cycle histogram; the steps marked "estimated" never ran in these songs and use the same cost rules
(no log checks them). Gaps without a host call in them are exact to the colour clock; the alternating 1 cc
differences seen in the raw logs (for example 99/100 cc between the two volume writes of channel 2) are the
half-cycle rounding of 6.1, not jitter.
