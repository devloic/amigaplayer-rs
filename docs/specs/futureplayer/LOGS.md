# Future Player: reference logs

Every log is a register-write log of the reference 68k player (the UADE Eagleplayer "Future Player" running
the unmodified module) run through this repository's host with `DELI_WRITE_LOG` set: one line per Paula
register write, `ADDRESS VALUE COLOUR-CLOCK` (hex address, hex value, decimal colour clock; the first line is a
comment with the module's load address). Audio went to `--wav /dev/null` with the dummy SDL driver and
`--player 'Future Player' --subsong N --seconds 200` was used. Names below are `<module>_s<subsong>.log`.
The three largest logs (`title_s0`, `ingame_s0`, `ingame_s1`) are cut to their first 30 000 lines; the checks
quoted below were made on the complete logs (8 796 and 3 091 ticks for the two in-game ones, 10 262 for
the title).

## Modules

| module | origin | subsongs logged | notes |
|---|---|---|---|
| `fp.title` ("Hybris title") | corpus, game music | 0 | 1 subsong, fade-in step 2, 4 voices, does not end |
| `fp.ingame` ("Hybris ingame") | corpus, game music | 0 to 5 | 0 does not end within 200 s; 1 to 5 end; 3, 4, 5 have pre-run counts 64, 128, 224 |
| `fp.syn_ctrl` | synthetic (built for this spec) | 0 to 5 | sequencer commands: nested calls, loops, jump, loop-forever, early end, absent voices, pre-run, fade steps, the 255-command abort, a song that ends during pre-run |
| `fp.syn_snd` | synthetic | 0 to 5 | waveform lists (including an empty one and a loop marker), period modulators (negate, shifts, stop flag, "do not restart" flag), "synth" waveforms with the two address modulators, one-shot waveforms, portamento (including the clamp cases), the default sound, rests and ties |
| `fp.syn_env` | synthetic | 0 to 5 | six envelope parameter sets covering every phase transition branch, fade-in variants (steps 1, 127, 128 with pre-run, 255), speed field 0 |

The synthetic modules contain only data (sound records, tracks, streams, song table); they were built
with a private generator and each was run against the reference in the same way.

## What the logs confirm

- A private model that follows SPEC.md reproduces, for every log, the address, the value, the order and the
  colour clock of every register write (address values up to one constant load offset per run, the
  silent-buffer address as the high word 0x0079):

| log | ticks | writes | differences |
|---|---|---|---|
| title_s0 | 10 262 | 160 622 | 0 |
| ingame_s0 | 8 796 | 108 966 | 0 |
| ingame_s1 | 3 091 | 48 676 | 0 |
| ingame_s2, s3, s4, s5 | 97, 97, 193, 109 | 1 588, 1 604, 3 156, 1 824 | 0 |
| syn_ctrl_s0 to s4 | 129, 1 539, 57, 1 385, 5 | 812, 3 974, 332, 2 807, 25 | 0 |
| syn_snd_s0 to s5 | 73, 73, 61, 65, 109, 49 | 892, 576, 804, 1 108, 948, 194 | 0 |
| syn_env_s0 to s5 | 201, 201, 161, 161, 149, 321 | 1 484, 1 388, 329, 329, 301, 649 | 0 |

- `syn_ctrl_s5` has no tick: the song ends during pre-run. The two voice-end writes (DMA control register
  = 1, then = 2) are 272 cc apart, which equals the model including the loop overhead of pre-run
  (section 8 of SPEC.md does not list it; it is `[pre-run overhead]` below).
- Tick spacing equals `5 * (256*T + 1)` exactly (no jitter) over all ticks of all logs.
- The raster-wait model of SPEC 6.10 reproduces every post-wait write, including the ticks where several
  voices trigger.

### Pre-run overhead (not in the SPEC cost table)

Start-up charges, before the first sequencer round: 20 + 9 + 20 cycles (a store, a branch, a store); then,
per round and per voice `i`: 12 + 18 cycles before the sequencer step of voice `i`, and 28 + 9 cycles at the
end of each round. When P is 0 these are not charged by the model (the constant is absorbed in K0). This only matters for the clock of voice-end writes (and the song-end event) during
pre-run, and for the clock of the first tick.

## Host start-up and stop sequences (not part of the replayer)

Observed in every log; the replayer's own writes start at the first tick.

1. First the host writes volume 0 to the four channels twice: 8 writes (about 150 to 170 cc apart,
   first one around cc 790).
2. Three identical "silence" rounds, each: DMA control register = 0x000F; then for channel 0 to 3 in
   order: address (silent buffer, both words at the same clock), length = 2, period = 1 and volume = 0 (the
   last two at the same clock). Offsets from the DMA write: channel 0 address +32, length +41, period/volume
   +54; each following channel +62 later. Rounds start about 580 and 1 082 cc apart (6349, 6929, 8011 in the first
   segment of the synthetic logs), the third ends at the last start-up write of the K0 table below.
3. With `--subsong N`, the host first initialises subsongs 0 to N-1 as stubs ("stub segments": the three
   rounds, then a stop sequence), then the real subsong N. A stop sequence is: DMA control register = 0x000F,
   then volume 0 to channels 0 to 3 at +8, +14, +20, +26 cc, then two further sets of four volume writes
   (about 230 cc later, 150 to 175 cc apart). The next segment starts roughly 4 000 to 6 000 cc after the stop.
   Only the last segment contains ticks; the tick-level checks above use that segment.
4. The log ends with the last write of the tick in which the song-end event fires (the host stops there);
   runs that do not end stop at the time limit.

## First tick (K0)

Tick `k` starts at `K0 + k*(5*(256*T+1))`. `K0` is derived from the first write of tick 0 and the model's offset of
that write inside the tick. Measured:

| log | subsong | pre-run P | fade F | voices | tick interval | last start-up write | K0 | K0 - last write - interval - pre-run cost |
|---|---|---|---|---|---|---|---|---|
| title | 0 | 0 | 2 | 4 | 69125 | 8013 | 77563 | 425 |
| ingame | 0 | 0 | 0 | 3 | 80645 | 8261 | 89378 | 472 |
| ingame | 1 | 0 | 0 | 4 | 71685 | 16469 | 88682 | 528 |
| ingame | 2 | 0 | 0 | 4 | 74245 | 24733 | 99506 | 528 |
| ingame | 3 | 64 | 0 | 4 | 74245 | 32997 | 130059 | 514 |
| ingame | 4 | 128 | 0 | 4 | 74245 | 63550 | 182222 | 514 |
| ingame | 5 | 224 | 0 | 4 | 74245 | 115713 | 266180 | 514 |
| syn_ctrl | 0 | 0 | 0 | 4 | 64005 | 8251 | 72784 | 528 |
| syn_ctrl | 1 | 0 | 0 | 3 | 57605 | 16525 | 74602 | 472 |
| syn_ctrl | 2 | 5 | 48 | 4 | 70405 | 24613 | 98481 | 414 |
| syn_ctrl | 3 | 0 | 0 | 2 | 64005 | 35832 | 100253 | 416 |
| syn_ctrl | 4 | 0 | 255 | 1 | 38405 | 43894 | 82556 | 257 |
| syn_snd | 0 | 0 | 0 | 4 | 64005 | 8251 | 72784 | 528 |
| syn_snd | 1 | 0 | 0 | 4 | 57605 | 16515 | 74648 | 528 |
| syn_snd | 2 | 0 | 0 | 4 | 64005 | 24779 | 89312 | 528 |
| syn_snd | 3 | 0 | 0 | 4 | 64005 | 33043 | 97576 | 528 |
| syn_snd | 4 | 0 | 0 | 4 | 64005 | 41307 | 105840 | 528 |
| syn_snd | 5 | 0 | 0 | 2 | 64005 | 49591 | 114012 | 416 |
| syn_env | 0 | 0 | 0 | 4 | 64005 | 8251 | 72784 | 528 |
| syn_env | 1 | 0 | 0 | 4 | 64005 | 16515 | 81048 | 528 |
| syn_env | 2 | 0 | 1 | 1 | 64005 | 24689 | 88951 | 257 |
| syn_env | 3 | 0 | 127 | 1 | 64005 | 32592 | 96854 | 257 |
| syn_env | 4 | 3 | 128 | 1 | 64005 | 40495 | 105636 | 242 |
| syn_env | 5 | 0 | 0 | 1 | 51205 | 49397 | 100966 | 364 |

Rule of thumb that reproduces `K0` to within about 15 cc: `K0 = last start-up write + interval + pre-run
cost + R`, with `R` = 528 cc for four voices, 472 for three, 416 for two, 364 for one (no fade-in, no
pre-run); R is about 104 cc smaller when fade-in is active, and 14 smaller when a pre-run exists. The
tick-internal clocks depend on `K0 mod 227` (raster wait); a replay that must be clock-exact therefore
needs the measured `K0` of the log it is compared with.

## Typical write offsets

First tick of `ingame_s1`, all four voices start a note (clock relative to the tick's first write, which is
2 589 cc after the tick start because the four sequencer steps run first):

| write | offset | gap |
|---|---|---|
| DMA off (channel 0) | 0 | |
| period 0 | 348 | 348 |
| length 0 | 604 | 256 |
| address 0 | 610 | 6 |
| volume 0 | 856 | 246 |
| DMA on (channel 0) | 2 988 | 2 132 |
| DMA off (channel 1) | 3 075 | 87 |
| period 1 | 3 441 | 366 |
| volume 1 | 3 865 | 273 |
| DMA on (channel 1) | 5 947 | 2 082 |
| channels 2 and 3 | same pattern | |

The 2 000 to 2 100 cc gap before each DMA-on write is the raster wait (9 line changes of 227 cc plus the
time to the next line).

## Not exercised by any log

See README.md.
