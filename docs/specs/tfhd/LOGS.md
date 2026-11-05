# Reference write logs

All logs come from the reference 68k player run through the host with DELI_WRITE_LOG: one
Eagleplayer in the player directory, empty DeliPlayer and DeliGenie directories, the WAV
output, the dummy SDL audio driver, 30 s, subsong 0. Lines are `ADDRESS VALUE COLOUR-CLOCK`.
The first line, `# module BASE SIZE`, gives where the file was loaded. The files are
xz-compressed.

Compare with `tools/replay-cmp.py REF.log PORT.log`. Sample addresses inside the TFHD file come
out as `M+offset`, where offset = H + M + (sample offset in the smpl part); see SPEC section 1.

## Real TFHD modules (the corpus holds only these two)

| log | player | module |
|---|---|---|
| real.tfhd1.5.rocknroll.TFMX-TFHD | TFMX-1.5-TFHD | Rock'n'Roll "Rockomotion", TFHD type 1 (mdat $2D80, smpl $17100 bytes) |
| real.tfhd7V.turrican2title.TFMX-7V-TFHD | TFMX-7V-TFHD | Turrican 2 title, TFHD type 3 (mdat $6BDE, smpl $13CE6 bytes) |

No real TFHD Pro module (type 2) exists in the corpus.

## Synthetic TFHD modules

Each one is a real `mdat.X`/`smpl.X` pair wrapped in an 18-byte TFHD header (H = $12, type =
1, 2 or 3, the two lengths) and nothing else. The log name gives the type (`_t1_`, `_t2_`,
`_t3_`) and the source pair. The same pairs wrapped as type 0, and the bare pairs, give logs
equal to these in every register value (SPEC section 7). Those logs are not kept here.

* 1.5: the nine Rock'n'Roll songs (spring dreams, take off song, magnetic beats, high times,
  rockomotion, lovely feelings, beach party, mouse control, jumping wild). These are the only
  "TFMX " (1.5) pairs in the corpus.
* 7V: Apidya title and game complete, Turrican 3 intro and title, B.C. Kid, Turrican 2 intro
  and title, Carl Lewis Challenge title (all 7-voice pairs of the corpus).
* Pro: 14 pairs picked at random from 197: X-Out ingame 4, Logical level 04, Grand Monster
  Slam ingame, Software Manager title, Hard'n'Heavy ingame, Abandoned Places outer 2 and
  level 2, Turrican 2 loader, The Power, Turrican 3 world 5, Patrician ingame, Circus
  Attractions ingame, Danger Freak, Mad TV ingame 6.
  * pro02, 04, 05, 07, 10, 11 and 13 use the fixed tables. The Eagleplayer TFMX_Pro V1.4
    refuses them; the repo's `Pro` engine plays them.
  * pro01 has a non-zero track mute word ($1CE = $00FF; SPEC 5.3).
  * pro06 starts with fade steps (SPEC 5.2).
  * pro10's song 0 is the empty entry (0, 0, tempo 0); SPEC 5.4 and 5.5.
* syn.pro06-loopcount2.TFMX-Pro-TFHD: pro06 with its final loop step's count changed from 0 to 2,
  in the two-file form, 200 s. The song ends at 38.5 s, at the first arrival at the loop (SPEC 5.4).

## Pairs used as the comparison side (not kept)

* The Eagleplayer TFMX_1.5 V1.91 (EP 2.06) on the bare 1.5 pairs; the repo's `V15` engine
  equals it tick for tick.
* The Eagleplayer TFMX_Pro V1.4 (EP 2.06) on the bare Pro pairs it accepts, plus the repo's `Pro`
  engine (`--open-replayers`) on all 14. The two are equal tick for tick where both play.
* The Eagleplayer TFMX_7V V2.35 (EP 2.06) on the bare 7V pairs; it is the same player as
  TFMX-7V-TFHD.

The timing tables in SPEC section 6 are medians over all ticks from the fourth on, taken from
these logs and the kept ones.
