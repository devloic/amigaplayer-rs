# TFMX single-file container (TFHD): behaviour spec

*Sanitised for publication: behaviour only.*

Three Eagleplayers play the single-file TFMX container (magic `TFHD`):

| Eagleplayer (UADE name) | prefixes | player's own name | engine it carries |
|---|---|---|---|
| TFMX-1.5-TFHD (file `TFMX-TFHD`) | `tfhd1.5` | TFMX_1.5, "TFMX 1.6 player module V1.9" (1995) | TFMX 1.5 |
| TFMX-Pro-TFHD | `tfhdpro`, `tfmx` | TFMX_Pro, "TFMX Pro Eagleplayer V2.36" (1995) | TFMX Pro |
| TFMX-7V-TFHD | `tfhd7V` | TFMX_7V, "TFMX_7V Eagleplayer V2.35" (1995) | TFMX 7 voices |

The repo's `src/replay/tfmx.rs` already plays the three engines from the two-file form
(`mdat.NAME` + `smpl.NAME`):

* `V15` follows the Eagleplayer TFMX_1.5 "V1.91" (1996),
* `Pro` follows the Eagleplayer TFMX_Pro "V1.4" (2009),
* `Seven` follows the Eagleplayer TFMX_7V "V2.35" (1995).

This spec gives (1) the container and how to get the two parts out of it, (2) each TFHD player's
detection rule, and (3) for each engine, how the TFHD player differs from the engine the repo
already follows. Summary:

* **7V**: the 7V-TFHD player is the same player the `Seven` engine follows (it plays the
  two-file form too). Only the container is new. Write logs of the same module are equal,
  value for value and to the colour clock, whether it is loaded as a TFHD file or as a pair.
* **1.5**: the same engine as `V15` (every register value equal on 9 modules), with three
  differences: DMA on is written per voice, inside the tick, instead of once at its end; the
  player writes Paula itself, so writes come sooner and closer together; extra writes at start.
* **Pro**: the same engine as `Pro` for notes, macros, effects, periods, sample pointers,
  lengths, interrupts and DMA (every register value equal on 14 modules once the changes below
  are made). The changes: start master volume $3F (not $40), song-start fades dropped, no
  track mute table, no "every track stopped" song end, a song end at every first pass of a
  track loop, its own subsong list (the module's song table is compacted), wider detection,
  no 9-line waits and different write spacing, its own CIA-B timer and start-up writes.

Sections 1 and 2 describe the container and detection, section 3 the 7V player, section 4 the
1.5 player, section 5 the Pro player, section 6 timing.

## 1. Container

All values are big-endian.

| offset | size | field |
|---|---|---|
| 0 | 4 | magic `TFHD` ($54464844) |
| 4 | 4 | header size H: the offset of the song part (always $12 in known files) |
| 8 | 1 | type: bits 0-6: 0 = unspecified, 1 = TFMX 1.5, 2 = TFMX Pro, 3 = TFMX 7V; bit 7 is ignored |
| 9 | 1 | 0 (unused) |
| 10 | 4 | song part length M (`mdat`) |
| 14 | 4 | sample part length S (`smpl`) |
| H | M | the `mdat` file, byte for byte |
| H+M | S | the `smpl` file, byte for byte |

* The players take the song part at file offset H (the value at offset 4) and the samples at
  H + M. They use the lengths in the header, not the file size: trailing bytes are ignored,
  and a different H (tested with $14) just moves both parts.
* Nothing is converted. The song part is read as the plain engine reads `mdat.NAME`, and sample
  offsets in the song are relative to the start of the sample part, as they are to the start
  of `smpl.NAME`. Splitting a TFHD file at H and H+M into `mdat.X` and `smpl.X` and playing
  the pair gives the same register values. The only change is the sample addresses, which
  move with where the parts are loaded.
* While loading, the players write the type byte back into offset 8 of the loaded file. The
  type-0 file then counts as the player's own type. Nothing else in the file changes at load,
  except as said in 5.6 (the Pro player rewrites its song table) and 3 (the 7V player fills in
  the default table offsets).
* Subsong numbers, title (none: TFMX has no song name field the players show), and song
  length come from the song part, as for the two-file form, but see 5.6 for Pro.

## 2. Detection (per player)

A TFHD file goes to a player when it starts with `TFHD` and its type (low 7 bits of byte 8) is:

* **the player's own type** (1.5: 1, Pro: 2, 7V: 3). The file is accepted without looking at
  the song part at all. Exception: the 7V player still fills in the default table offsets (see
  3).
* **0**: the player skips H bytes and applies its plain-file check to the song part.
* **any other value**: rejected.

The plain-file checks, also used for files without the TFHD header:

* **1.5-TFHD**: the data starts with `TFMX` and bytes 5..8 are not `SONG`. So "TFMX " and
  "TFMX-SONG " modules are told apart only by these four bytes.
* **Pro-TFHD**: the data starts with one of `tfmxsong`, `TFMX` (so any TFMX text, including
  the 1.5 "TFMX " form), or `TFMX-Song`. Then:
  * reject when the long at $1D0 is 0 and the long at 4 is 0;
  * take the step at (long at $1D0) + 16 × (word at $100). For a module of fixed tables
    (long at $1D0 = 0) this reads at offset 16 × (word at $100), not at $800 (a quirk of the
    original). Reject when that step is the 7-voice step $EFFE 0003;
  * accept otherwise.
  * Only that one step is looked at. A type-0 TFHD file holding a 1.5 module is taken by this
    player, so a host should try the 1.5 player first (UADE does this through the prefixes).
  * This accepts fixed-table modules (offset $1D0 = 0) that the Eagleplayer TFMX_Pro V1.4
    declines. The repo's `Pro` engine accepts them already.
* **7V-TFHD**: the data starts with `TFMX`, the second long of the pattern table and the
  second long of the macro table are not 0, and some step from the first song's start step
  on is the 7-voice step `$EFFE 0003`. The steps from the start that are `$EFFE` commands are
  walked; the first non-`$EFFE` step rejects. The tables are at the offsets in $1D0/$1D4/$1D8,
  or $800/$400/$600 when the long at $1D4 is 0. This is the repo's `detect_7v` rule.
  * When the module uses the fixed tables, the player writes $800, $400, $600 into $1D0, $1D4,
    $1D8 of the loaded module before playing. When the check fails, it puts back zeros if the
    value at $1D0 was $800. This side effect only matters to code that reads those header
    words.

## 3. TFMX-7V-TFHD

The engine is the one `Seven` follows. The player file is the Eagleplayer TFMX_7V V2.35 with only
some name text changed, and both take the TFHD form and the pair. A player for TFHD 7V only
has to take the parts out of the container (section 1) and run `Seven`. Checked on the real
TFHD module and 6 synthetic ones (section 7): the write logs are equal to the colour clock
whether the module is given as a TFHD file, a type-0 TFHD file or an `mdat`/`smpl` pair.
The only shift is a constant of at most 160 colour clocks (either way) for the whole song after the first ~53
writes, from the different load path, which moves the start phase.

## 4. TFMX-1.5-TFHD

### 4.1 Same as the `V15` engine

Track steps, patterns, macros, effects, periods, volumes, sample pointers and lengths, subsong
range (the same count from the song table) and song end (the same times on all 9 modules, up to
236 s) are the `V15` engine's. Every register value of every tick is equal to the `V15`
reference once 4.2 is applied (9 modules, about 1 500 ticks each).

### 4.2 DMA on

`V15` collects the DMA-on bits of a tick and writes one DMACON = $8000 | mask at the very end of
the tick.

The 1.5-TFHD player writes one DMACON per voice, in the same tick:

* the value is **$8200 | (1 << voice)**: the master DMA enable bit is set, unlike `V15`'s
  value;
* it is written right after that voice's volume write of the tick, before the next voice's
  writes begin;
* voices come in order 0, 1, 2, 3, and a voice turned on in the tick writes its own DMACON;
* there is no combined write at the end of the tick, and no wait for DMA after it.

So a tick in which `V15` writes `... VOL1 ... VOL2 ... VOL3 DMACON=$8006` is, in 1.5-TFHD,
`... VOL1 DMACON=$8202 ... VOL2 DMACON=$8204 ... VOL3`. The DMA-off writes (DMACON = 1 << voice)
are unchanged in value and position.

### 4.3 Start

* At init, three times, VOL0..VOL3 = 0. The four writes of a group are 136, 148 and 143 colour
  clocks apart; the groups start 804 and 2 397 colour clocks after the first.
* On the first timer interrupt, the tick before the first tick in which `V15` writes
  anything: VOL0..VOL3 = 0 (10 colour clocks apart), DMACON = $000F 10 later, ADKCON = $00FF
  1 113 later.
* From then on, ticks come at the same moments as `V15`'s (same timer, same tempo handling).

### 4.4 Spacing

See 6.2. Every tick's first write is 2 222 colour clocks earlier than `V15`'s first write of the
same tick (median; 2 222..2 233 in 85 % of ticks, 2 297 in most of the rest). The writes inside
a tick are closer together, because the player pokes Paula directly rather than through the
host's poke functions.

## 5. TFMX-Pro-TFHD

### 5.1 Same as the `Pro` engine

These are unchanged: track steps (including the fade lock, which ignores a fade command while
another runs), patterns, macros, periods, effects, sample pointers and lengths, the per-voice
interrupt handling and its INTENA/INTREQ writes, DMA off at the macro's command, DMA on and
periods at the end of the tick. Every register value of every tick from the first music tick
on equals the `Pro` engine's, on 14 modules, once 5.2 to 5.5 are applied.

### 5.2 Master volume

* The volume out is `Pro`'s rule: when the master volume is under 64,
  out = (voice volume × 4 × master) >> 8, with the product cut to 16 bits before the shift.
  Otherwise the voice volume goes out unscaled. The host volume and balance then scale it the
  same way as in `Pro` (no effect at 64/64).
* **At song start the master volume is $3F, not $40.** With no fade running, every volume
  written is therefore (v × 63) >> 6, one less than `Pro`'s for any v from 1 to 64.
* **Fades from the song's first track steps are dropped.** The song start reads the first
  track step(s) before the player's volume setup, which then sets the master to $3F at once
  and clears the fade lock and direction. So a song starting with
  `$EFFE 0004 0000 0000 / $EFFE 0004 0008 0040` (master to 0, then fade in) plays at master
  $3F from the first tick in TFHD-Pro, where `Pro` fades in from silence.
* A fade begun by a pattern command in the first tick (after the setup) runs as in `Pro`. A
  module that sets the master to 0 and fades up from a pattern plays the same in both.

### 5.3 No track mute table

`Pro` reloads, every row, eight track mute words from the song part's offsets $1C0..$1CF. A track
whose word is not 0 keeps running but its notes are not played. The TFHD-Pro player does not
read $1C0..$1CF: every track plays. One of the 14 test modules has $00FF in the eighth track's
word (offset $1CE); its eighth track is silent in `Pro` and plays in TFHD-Pro.

### 5.4 Song end

TFHD-Pro signals song end in these cases:

* **a track-step stop command** (`$EFFE 0000`);
* **the first time a track-loop step (`$EFFE 0001`) is met**, that is, when its loop counter
  is not running, **whatever the loop count**. The loop then runs as in `Pro`: counter = count
  - 1, jump to the target step. `Pro` only signals song end there when the count is 0 or
  negative (endless). Test: one module's last step, a loop with count 0, was changed to count
  2. TFHD-Pro ended at the first arrival there (38.5 s); `Pro` and the Eagleplayer TFMX_Pro
  did not end within 200 s;
* **a pattern end (`$F0`) while the track position is the song's last step** (the position
  goes back to the first step), as in `Pro`.

`Pro`'s extra check at each tick, "song end when all eight tracks have no pattern", does not
exist in TFHD-Pro. A song whose patterns all stop just keeps ticking silently or with held notes.
Example: a song whose only step plays one pattern: `Pro` ends after 4 ticks, TFHD-Pro plays on.

### 5.5 Subsong list (the module's song table is rewritten)

At load, the player compacts the song table of the song part. The table has 32 entries:
start step words at $100, last step words at $140, tempo words at $180. It walks entries
i = 0..30 (entry 31 is only looked at as a neighbour):

1. Take (start, last) of entry i. While entry i+1 has the same start and the same last (and
   i+1 ≤ 30), move on to it. Count this as a run of duplicates.
2. Keep the entry (the last of the run, but all are equal) when any of these holds:
   * start ≠ last;
   * start = last, there were no duplicates, and start ≠ 0;
   * start = last = 0, there were no duplicates, and its tempo is neither 0 nor 5.

   Otherwise drop it. (start = last with duplicates is always dropped.)
3. Kept entries are copied to the front of the three tables, in order, start/last/tempo
   together. The rest of the tables is left as it was.

The subsong range is 0 .. (number kept - 1), at least 0..0. Subsong n plays kept entry n.
Checked on all 14 modules: the counts match the player's (`Pro`'s own list differs on 3 of
them). Played subsongs were checked for two (e.g. TFHD-Pro subsong 1 of a module whose entry 0
is (0, 0, tempo 0) plays the original entry 2).

The player also clears byte $FF of the song part at load (the last byte of its text area).

### 5.6 Start-up writes

TFHD-Pro runs from its own CIA-B timer A interrupt. The default timer value is $376C (14 188 E
clocks = 70 940 colour clocks a tick; the mean measured tick is 70 988, the same as `Pro`'s).
Tempo track steps set the timer value as in `Pro` ($1B51F8 / value).

The start, in order (times are colour clocks; where a range is given it varied between runs):

1. INTENA = $0780 (audio interrupts off), at load time.
2. Much later, at InitSound: VOL0..3 = 0 (139, 150, 143 apart); 1 573..2 406 later
   VOL0..3 = 0 again (same spacing).
3. 4 035..5 674 later: DMACON = $000F; +12 INTREQ = $0780; +18 INTENA = $0780; +22
   INTREQ = $0780; +30, +38, +46, +54 VOL0..3 = 0; +262 CIA-B timer A high = $37; +272
   timer A low = $6C; +318 INTENA = $A000 (master enable plus the CIA-B line); then the audio
   filter off (CIA-A port A bit 1 set) at +1 175..+1 330, ADKCON = $00FF 10 later, and
   VOL0..3 = 0 about 430 later (139, 150, 143 apart).
4. CIA-B timer A started 1 329..2 161 after that last VOL group.
5. The first tick's first write comes 79 037 colour clocks after the timer start (equal within
   1 on three modules). From that tick on, every register value is `Pro`'s first music tick
   and onwards, with 5.2 to 5.5 applied. `Pro`'s own start (VOL0..3, DMACON $000F,
   INTREQ/INTENA $0780, ADKCON $00FF, all within about 1 000 colour clocks) is replaced by
   the above.

`Pro`'s first tick (all voices off: VOL0..3 = 0, INTENA/INTREQ per voice, PER0..3 = 0,
DMACON = 0) is also TFHD-Pro's first tick. TFHD-Pro adds nothing else.

## 6. Timing

### 6.1 7V

Identical to `Seven`'s reference (same player). Nothing changes.

### 6.2 1.5-TFHD: per-write gaps

Median colour clocks from one write to the next inside a tick, by register kind (9 modules,
~13 000 ticks), next to `V15`'s reference. p10..p90 are in brackets.

| from -> to | 1.5-TFHD | `V15` reference |
|---|---|---|
| VOL -> VOL | 364 (350..461) | 368 (358..609) |
| PER -> VOL | 359 (271..377) | 359 (278..370) |
| VOL -> PER | 238 (141..374) | 307 (236..455) |
| LOC -> LEN | 203 (96..248) | 270 (164..316) |
| VOL -> DMACON (off or per-voice on) | 152 (128..152) | 232 (117..232) |
| LEN -> PER | 353 (236..376) | 423 (306..446) |
| VOL -> LOC | 159 (159..185) | 219 (219..245) |
| DMACON -> LOC | 154 (154..155) | 232 |
| DMACON(on) -> next voice's VOL | 462 (455..473) | (no such write) |
| LEN -> VOL | 476 (469..597) | 477 (471..603) |
| PER -> PER | 149 (140..153) | 216 (216..220) |
| DMACON -> PER | 236 (222..313) | (rare) |
| LOC -> VOL | 521 | 520 |
| LEN -> LOC | 168 | 228 |
| LOC -> PER | 298 | 370 |

What the numbers show:

* Each Paula write in 1.5-TFHD costs about 60..80 colour clocks less than in `V15`. VOL->VOL and
  LEN->VOL do not change, because those gaps are spent in the engine, not in the write.
* The first write of a tick is 2 222 earlier (4.4).
* The DMA on written after a voice's volume comes 128 (a voice whose note started this tick)
  or 152 colour clocks after it.

### 6.3 Pro-TFHD: per-write gaps

Same layout (7 modules of variable tables, ~10 000 ticks), next to the Eagleplayer TFMX_Pro V1.4
reference, which `Pro` matches tick for tick.

| from -> to | Pro-TFHD | `Pro` reference |
|---|---|---|
| PER -> PER | 28 | 16 |
| VOL -> VOL | 551 (491..721) | 436 (383..912) |
| PER -> DMACON (end of tick) | **20** | **2 011** (1 912..2 099) |
| VOL -> PER | 82 (82..670) | 186 (186..824) |
| INTENA -> INTREQ | 12 | 12 |
| LOC -> LEN | 284 (203..333) | 161 (16..196) |
| LEN -> VOL | 526 (385..564) | 548 (318..698) |
| VOL -> LOC | 388 (322..459) | 286 (231..2 361) |
| VOL -> INTENA | 351 (325..671) | 264 (242..786) |
| INTREQ -> VOL | **382** (360..528) | **2 414** (2 260..2 557) |
| DMACON(off) -> LOC | **134** | **2 184** (2 072..2 247) |
| LOC -> VOL | 421 (418..681) | 312 (312..445) |
| VOL -> DMACON | 397 (371..409) | 328 (313..335) |
| INTREQ -> INTENA | 541 (541..729) | 565 (565..670) |
| INTREQ -> PER | 440 (129..578) | 472 (361..623) |
| LEN -> INTENA | 426 (410..571) | 592 (561..761) |
| DMACON -> PER | 29 (20..48) | 29 (20..48) |
| PER -> VOL | 677 (556..774) | 461 (384..602) |
| PER -> LOC | 409 (373..427) | 2 221 (2 145..2 329) |
| INTREQ -> LOC | 330 | 2 418 (2 328..2 521) |
| LOC -> LOC | 379 (351..379) | 280 (276..307) |

The bold rows are the structural difference: **TFHD-Pro has none of `Pro`'s 9-raster-line
waits.** Three places have them in `Pro`: before the end-of-tick DMA-on write, after a macro's
DMA off before the sample pointer, and after the wait-on-DMA interrupt handling before the
next write. In TFHD-Pro, the end-of-tick DMACON comes 20 colour clocks after PER3, and the
sample pointer 134 after a DMA off.

The other costs are different code with a similar shape: the period block is written 28 apart,
not 16, the voice code costs more (VOL->VOL), and the poke costs less (VOL->PER). The DMA-off
to DMA-on distance still spans a tick boundary, as in `Pro`.

An exact per-path cost model was not fitted (see README). Start from `Pro`'s cost model, remove
the three waits, and fit to the logs.

## 7. What was checked, and how

* Container (section 1): 29 synthetic TFHD files, 14 Pro, 9 1.5 and 6 7V, each built as a
  type-N and a type-0 file from a real `mdat`/`smpl` pair, plus the 2 real TFHD modules. The
  TFHD player's log for the TFHD file equals its log for the split pair, and the type-0 log
  equals the type-N log: no value differs, sample pointers differ by a constant, and the
  timing equals or is shifted by a constant.
* 1.5 (section 4): a model built from this text (take the `V15` reference log and apply 4.2)
  equals the 1.5-TFHD log in every register value and in the order of writes, every tick from
  the first music tick: 9 of 9 modules, 0 differing ticks of ~13 400.
* Pro (section 5): a model built from this text equals the TFHD-Pro log in every register
  value and order, every tick from the first music tick, on 13 of 14 modules (0 differing
  ticks of ~19 000). The model: the `Pro` engine run on the module with the $1C0 words zeroed
  and the song-start fade steps made "master $40 at once", then every volume taken as
  (v × 63) >> 6. The 14th module fades the master from 0 by a pattern command in its first
  tick, so its volumes must not be rescaled: it is equal with the volumes taken unchanged (0
  differing ticks). Of the 13, pro10 counts only weakly: `Pro`'s mapped subsong ends after 4
  ticks because of `Pro`'s "every track stopped" rule (5.4), so only 4 ticks could be
  compared. Its TFHD-Pro subsong 1 (= raw entry 2, mapped per 5.5) is equal for the 194 ticks
  `Pro` plays of it.
* 7V (section 3): the same player, so the logs are equal.
