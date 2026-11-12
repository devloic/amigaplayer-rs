# Rob Hubbard 2 (`rh2.*.dum` + sample bank): behaviour spec

Scope: the "Rob Hubbard 2" module family (UnExoticA: Stir Crazy featuring Bobo, 8 modules; Captain
Blood, 1 module), each module a small data file beside a sample bank `rh.ins`. A separate family from
the Rob Hubbard `rh.*` driver images (`docs/specs/robhubbard/`) and from Rob Hubbard ST (`rho.*`).

Method: black box only. The reference is DeliTracker's DeliPlayer `RobHubbard_2` (PlayerStore), a
NotePlayer player, played through DeliTracker's 4-voice NotePlayer genie (`4Voices-NotePlayer`) on
this project's 68000 engine. Observed: the Paula register writes with their colour clocks
(`DELI_WRITE_LOG`), the audio words Paula fetched (`DELI_DMA_LOG`), which module and bank bytes the
guest read and when (a read watch in a private engine copy), and the output of built modules (the
layout of section 2 with edited patterns, envelope records and lengths: differential probing).
Neither the player nor the genie was disassembled or read. No code or data of either is in this text;
the period and length tables are the values seen played.

Times are colour clocks ("cc", 3 546 895 a second). A tick's writes are timed from the tick's start
(the timer interrupt) unless said otherwise.

## 1. Reference

| item | finding |
|---|---|
| Eagleplayer 2.06 "Rob Hubbard 2" | asks (dos.library, through `dtg_LoadFile`) for `<module>.ins`, then `<module less its last character>.ins`, then `rh2.ins`; with the archive's `rh.ins` only: "could not load additional files". With a copy named `rh2.ins` it starts, then returns from its interrupt with an unbalanced stack (SP 8 bytes below the entry's) and writes nothing: silent, not usable |
| DeliPlayer `RobHubbard_2` | loads the bank from the module's directory as `rh.ins`, the archive's name, and asks for no other name (with no bank it fails); plays all 9 modules; never reports a song end (after the last voice ends it writes period 0, volume 0 on all voices every tick, for ever) |
| genie | the NotePlayer genie writes Paula; without a genie the engine's built-in note renderer plays instead, which is not the reference |

## 2. Module layout

All words big-endian. Offsets stored in the header are file offsets **less 2**.

| offset | size | meaning |
|---|---|---|
| 0 | word | 2 in every module (not varied) |
| 2 | word | speed: ticks per step (2 to 5 in the corpus) |
| 4 | word | volume envelope table (A) − 2 |
| 6 | word | period envelope table (B) − 2 |
| 8, 10, 12, 14 | words | track list of voice 0..3 − 2 |
| 16 | word | file length − 2 |
| 18 .. track 0 | words | pattern offsets − 2, one per pattern (pattern count = (track 0 − 18) / 2) |

Then, in this order and without gaps: the four track lists (pattern numbers, each list closed by
`FF`), the patterns (byte streams, each closed by `FF`), table A (32 records of 13 bytes), table B (32
records of 13 bytes), end of file. In all 9 modules A + 416 = B and B + 416 = file length, the
offsets increase in the order listed, and every list and pattern ends with `FF`.

Detection (used by the port; the reference's own check was not probed): the word at 0 is 2, the speed
is 1 to 255, the layout above holds exactly (A + 416 = B, B + 416 = length = word 16 + 2, offsets
strictly increasing, every track list and pattern ends in `FF`, every track-list entry is a pattern
number below the pattern count), and a bank (section 3) is found beside the module.

## 3. Sample bank

| offset | size | meaning |
|---|---|---|
| 0 | long | bank length − 4 |
| 4 + 16 n | 16 bytes | entry of sample n: long start, long loop start, long (equal to loop start in both banks; no effect seen), word length in words, word loop length in words |

Starts are bank offsets − 4 (data at start + 4). The entry table ends where sample 0's data starts
(19 entries in Stir Crazy's bank, 10 in Captain Blood's). A sample number past the table reads its
"entry" from what lies at 4 + 16 n (sample data): Stir Crazy's `bobomang3` plays samples 25 to 27;
the writes and fetched words of those notes equal the reference's when the 16 bytes there are taken as
an entry and the block they describe is played if it lies in the bank.

Playback of a sample (seen in the fetched words): its `length` words from the start, then the loop
(`loop length` words from the loop start) repeated, until the voice's next sample start. A loop
length of **1 word plays silence** (zeros) in the reference, not the word (Stir Crazy samples 1, 3,
8, 10, 16; the fetched words after their end are 0). Loops of 2 words exist only with zero data.

## 4. Patterns

A voice reads its track list: each byte is a pattern number; `FF` ends the voice (it is silent from
then on: period 0, volume 0 written every tick). A pattern is read byte by byte:

| byte | meaning |
|---|---|
| `00`-`7F` | note: ends this tick's reading (section 7) |
| `80`-`9F` | length of the following notes, index = low 4 bits (`9x` = `8x`, probed) |
| `A0`-`BF` | sample = low 5 bits; the next note starts it |
| `C0`-`DF` | volume envelope = record (low 5 bits) of table A |
| `E0 xx` | transpose = xx (a byte added to the note) |
| `E1 xx` | period envelope = record xx of table B |
| `FF` | end of pattern: the next track-list entry |
| `E2`-`FE` | **unobserved** (no module has them, not probed) |

Lengths in steps (probed at speed 2 and 3, each code played twice): `80` 2, `81` 3, `82` 4, `83` 6,
`84` 8, `85` 12, `86` 16, `87` 24, `88` 32, `89` 48, `8A` 64, `8B` 96. Past the corpus (`8C`-`8F`,
unused by any module): `8C` 8 steps; `8D`, `8E`, `8F` gave 1 tick or 766 ticks depending on what was
played before: the reference reads memory past its table there.

Note: index = (note + transpose) & 255, except note `00`, which ignores the transpose (probed:
transpose 1, FF, FE with note 0 all give index 0). Index 0 to 101 is the period table (section 6);
above 101 the reference reads past its table (probed values unrelated to the table). A note sets the
voice's period, its length counter to the current length, and restarts both envelopes (section 5).
A note starts the sample only if a sample byte came since the voice's previous note; otherwise the
sample plays on (no DMA restart), only period and envelopes change. Initial state of a voice: length
code `80` (2 steps), transpose 0, envelopes record 0 of each table, sample 0, nothing armed.

## 5. Envelopes

A record of 13 bytes: byte 0 flags, then 4 steps of 3 bytes (count, delta, ticks). An envelope has a
value (starts at 0 at each note), a step (0 to 3), a tick counter and a repeat counter (both 0 at the
note). Every tick of a live voice, for each of its two envelopes, while the step is below 4:

1. if the tick counter is 0, the value += delta (signed byte);
2. tick counter += 1; when it equals `ticks`: tick counter = 0, repeat counter += 1; when that equals
   `count`: repeat counter = 0, step += 1;
3. after step 3 (step becomes 4): the period envelope always goes back to step 1; the volume envelope
   goes back to step 1 only if bit 7 of byte 0 is set (probed: 80 loops; 00, 01, 40 stop), else it
   ends and its value stays.

Counters are bytes (a count or ticks of 0 means 256). Byte 0 of a period record had no effect
(00, 7F, 80 probed). The read order seen: delta, ticks, count of the current step each tick.

Output: volume = volume envelope value (negative → 0; no module goes above 64); period = note period
+ period envelope value, converted as in section 9.

## 6. Period table

Indices 0 to 101 (each found as the only internal period that gives every written period of a 288-tick
slide of −1 a tick through the conversion of section 9):

```
27340 25805 24357 22990 21699 20483 19334 18247 17222 16267 15347 14482 13672 12905 12179 11498
10854 10241  9574  9125  8623  8132  7678  7246  6844  6454  6092  5750  5427  5121  4836  4565
 4308  4067  3838  3622  3419  3227  3045  2875  2715  2562  2418  2281  2153  2033  1919  1811
 1709  1613  1522  1437  1357  1280  1208  1141  1077  1016   959   906   854   806   761   719
  678   640   604   570   538   508   480   453   427   403   381   359   339   320   302   285
  269   254   240   226   214   202   190   180   170   160   151   143   135   127   120   112
   96    80    64    48    32    16
```

## 7. Tick

The timer runs at 3546895 / 35525 cc = 99.84 ticks a second (timer value 7104). Each tick:

1. speed counter −= 1; at 0 it is reloaded with the speed and this tick is a step;
2. for voice 0 to 3: on the first tick every voice reads to its first note; on a later step tick each
   live voice decrements its length counter and reads to its next note when it reaches 0;
3. then, for each live voice, both envelopes (section 5);
4. a voice whose sample started 2 ticks before (and not again in this tick) has its loop set
   (section 8);
5. the writes (section 8).

The reference's song never ends (section 1). The port ends the song once all four track lists
reached `FF`; up to that tick every write of the reference is reproduced.

## 8. Write order of a tick

Without a sample start: for voice 0 to 3: period, volume. Gaps: first period 187 cc after the tick's
first write time (189 when the period is 0), then volume +141, next period +187 (189 for period 0).

With sample starts (mask S):

1. INTENA off for the audio interrupts (genie; all four bits in the logs seen); period 1 on each voice of S (+18 cc apart);
   INTREQ clear; DMACON off S;
2. for voice 0 to 3: period, volume; a voice of S then writes its sample pointer and length and its
   period again (the genie's buffer, section 9). Gaps (reference typical): 300 cc before the first
   voice, 594 after a starting voice, 556/230 otherwise; volume +141; pointer +58; period again +7;
3. INTREQ clear, DMACON on S, INTENA on S.

Loop set (2 ticks after a start): the voice's next buffer takes the loop (section 9).

The reference also has a genie start-up tick before the player's first one (DMA off, volume 0 twice,
period 128, DMA on): the port does not write it.

## 9. What the genie adds (the reference's, not the format's)

1. Period: the player's period P (P > 0) reaches Paula as `C div f` with `f = (2·C + P) div (2P)`
   (the frequency rounded to nearest), C = 3546895, never below 122; P = 0 is written as 0. Chosen
   among floor/round/ceil variants and nearby constants as the only one that gives every written
   period of the slides of section 6.
2. Samples: the genie plays every sample from two 96-byte buffers per voice, filled from the sample
   one buffer ahead at each audio interrupt (pointer write, then INTREQ clear 367-480 cc later).
   The sample's bytes, then from the buffer after the loop is set the loop's, reach Paula at the
   period's rate: for every sample longer than two buffers the fetched words equal those of playing
   the sample directly with Paula's pointer and length, then the loop. A shorter one (Stir Crazy
   sample 6, 7 words) plays from the buffer, then zeros to the buffer's end, until the loop is set.

## 10. Times

The time from the tick's start to its first write depends on what the tick did. A linear fit of the
reference's first-write times on counts of the player's work (per tick: step tick, voices that read,
track-list bytes, pattern ends, notes, length bytes, sample bytes, volume and period envelope
selects, transposes, sample starts, live and ended voices, envelope additions, repeats, step changes
and loops, loops set) has, over the 54 691 ticks of the 9 modules where no genie interrupt falls in
that time: mean error 8 cc, 86 % within 8 cc, largest 176 cc. Weights (cc): base 280, step 50, read
87, track byte 65, pattern end 41, note 151, length 100, sample byte 83, volume select 208,
transpose 129, period select 258, sample start 83, live voice 753, ended voice 365, volume add 19,
volume repeat 45, volume next step 76, volume ended −25, period add 17, period repeat 42, period
next step 73, period loop 110, loop set 104.

Not reproducible from the player's side: a genie audio interrupt (buffer refill) or another host
interrupt between the tick's start and a write delays it (by 41, 58 or several hundred cc); this
happens in about 30 % of the ticks.

## 11. Unobserved / not established

- Commands `E2`-`FE`, length codes `8C`-`8F` (past the table), notes past index 101, sample numbers
  whose "entry" points outside the bank: no module has them; the reference reads its own memory.
- Volume envelope values above 64 or below 0 (none in the corpus).
- Byte 0 of a module (always 2), the third long of a bank entry.
- Subsongs: none (the reference reports subsong 0 only).

## 12. Check of the port against the reference

`src/replay/robhubbard2.rs` follows this text. Its writes against the reference's, each module
rendered for 180 s (the port stops at its song end, 3.3 s to 145 s), compared up to the port's end:

| module | music writes (period, volume, DMACON) | equal ticks | in-tick time exact / ≤ 8 cc | absolute time ≤ 8 cc / ≤ 64 cc |
|---|---|---|---|---|
| bobodors | 68 812 | 8 456 of 8 456 | 95.3 % / 96.3 % | 78.0 % / 87.0 % |
| bobofil | 51 558 | 5 960 of 5 960 | 81.1 % / 84.9 % | 65.8 % / 76.0 % |
| bobolav3 | 78 570 | 9 195 of 9 195 | 83.2 % / 86.5 % | 67.8 % / 78.5 % |
| bobomang3 | 77 298 | 9 225 of 9 225 | 87.6 % / 89.7 % | 74.0 % / 84.4 % |
| bobopat3 | 118 848 | 14 097 of 14 097 | 87.1 % / 88.9 % | 72.1 % / 82.9 % |
| bobotram | 69 046 | 8 106 of 8 106 | 85.1 % / 87.9 % | 71.3 % / 80.1 % |
| interlude | 2 692 | 329 of 329 | 92.9 % / 93.7 % | 54.0 % / 93.1 % |
| scores | 67 788 | 8 066 of 8 066 | 88.4 % / 90.0 % | 74.7 % / 81.7 % |
| blood | 116 748 | 14 516 of 14 516 | 98.8 % / 99.0 % | 92.9 % / 93.6 % |

Value and order: every one of the 651 360 music writes equals the reference's (the genie's start-up
tick of section 8 aside). In-tick time: the time from the tick's first write. Absolute: after one
constant shift per module (−1 to 11 cc). Exactly equal absolute times: 9 to 25 %. Not compared: the
sample pointers, lengths and interrupt registers (the genie's buffers against the port's direct
playback, section 9); instead the audio words Paula fetched, note by note, first 30 s: equal in
7 modules, 99.90 % (bobofil) and 99.97 % (bobolav3) of the words in the other two, the difference
being the buffer in which the short sample 6's loop begins (genie interrupt timing, 4 notes).
