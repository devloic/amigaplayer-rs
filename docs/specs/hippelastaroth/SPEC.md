# Hippel "Astaroth" replayer (astaroth.hip) - behaviour specification

*Behaviour only: written from black-box runs of the reference and from the module's own
embedded replayer, which is module data. No player code is reproduced here.*

Slug `hippelastaroth`. The module is `astaroth.hip` (28924 bytes, Jochen Hippel, "Astaroth", a
game rip): Jochen Hippel's replayer code followed by three `COSO` blocks. The reference is the
Eagleplayer "Hippel" (Eagleplayer 2.06), which calls the replayer the module carries, run on the
project's 68000 host (`--features host-68k`, `--68k-players --player Hippel`, an Eagleplayers
directory holding only "Hippel", `DELI_WRITE_LOG`).

Numbers are decimal unless written `$` (hexadecimal). "cc" = colour clocks (3.546895 MHz, 227 a
raster line); "cycles" = 68000 CPU cycles (2 a colour clock). Module offsets are offsets in the
file (the reference loads it at `$45E0`; every address a write carries that lies in the module is
`$45E0 + offset`). Words and longs are big-endian.

## 1. Overview

* The song data uses the COSO packing (offset tables, variable-length patterns, 10-byte sample
  entries), but the replayer is an early one, closer to the "early" TFMX builds of `hippel.rs`
  than to the Hippel-COSO Eagleplayer's replayer that `hippel.rs` `Coso` ports. Section 10 lists
  the differences from both.
* Two songs: song A (COSO block at `$9F4`, three reachable subsongs) and song B (block at
  `$20D2`, one song). A third block (`$33B8`) holds the sample table; the sample bytes start at
  `$3888`.
* One tick per interrupt, every 70940 cc (a CIA timer, 50.0 Hz). A tick reads the rows when the
  song's speed counter runs out, then for each voice 0..3 runs its effects and writes its period
  and volume, then turns the DMA on (every tick, with whatever bits the tick collected) and writes
  all four voices' loops.
* No song end: the position list wraps to the subsong's first position for ever.
* The replayer also has a sound-effect part (a second "song" in the sample block, its own entry
  points). The reference never calls it; it is not part of this spec (section 4.4).

## 2. File layout and detection

### 2.1 Fixed layout of this file

| Offset | Size | What |
|---|---|---|
| `$0000` | `$7EA` | replayer code. Entry points: `$0000` InitSound (`d0` = song number, section 4), `$0002` the tick, `$0004` the effects tick (unused) |
| `$07EA` | 8 | the "empty table": `01 00 00 00 00 00 00 E1` (used as a sample, a volume sequence and a frequency sequence before a voice's first note) |
| `$07F2` | 4 x `$48` | the four voice records (section 6); the file holds values saved from the game, three of which matter (6.2) |
| `$0912`..`$094D` | | the replayer's pointers and counters (rewritten by InitSound) and a table of the voices' register offsets and DMA bit numbers (`$926`: `$00A0,0  $00B0,3  $00C0,6  $00D0,9`, the second word being 3 x the voice number) |
| `$094E` | 83 words | period table (section 8). Notes 83..127 read on into the bytes after it (into song A's block) |
| `$09F4` | | song A: `COSO` block |
| `$20D2` | | song B: `COSO` block |
| `$33B8` | | sample block: `COSO` block; its long at `+$18` is the offset (from `$33B8`) of the sample table (here `$386`, so the table is at `$373E`, 33 entries) |
| `$3888` | to the end | sample bytes (the sample table's offsets count from here; `$3888` = table + 33 x 10) |

The code takes these addresses from PC-relative `lea` instructions, so an implementation that
wants to be general can read them from the code (the instruction words are allowed search
patterns): `lea d16(pc),a0` = `41FA dddd`, the target being the address of the displacement
word + `dddd` (signed):

| Instruction at | Bytes | Target |
|---|---|---|
| `$0052` | `41FA 3364` | sample block `$33B8` (followed by `2268 0018`: its `+$18` long) |
| `$006E` | `41FA 0984` | song A `$09F4` |
| `$0196` | `41FA 1F3A` | song B `$20D2` |
| `$01F0` | `43FA 3696` | sample bytes `$3888` |
| `$0726` | `43FA 0226` | period table `$094E` |
| `$00B4` | `41FA 073C` | first voice record `$07F2` |
| `$00B8` | `43FA 0730` | the empty table `$07EA` |

### 2.2 Detection

The file is recognised by its code (the replayer build): the FNV-1a 32-bit hash (offset basis
`$811C9DC5`, prime `$01000193`, over bytes; the `fnv` function of `hippel.rs`) of the bytes
`[0, $7EA)` - from the file start to the empty table, which `hippel.rs` already finds by its 8
bytes - is `$123E2CDA`. Structural checks that hold as well, if a looser test is wanted:

* the words at 0 and 2 are `601E 600E` (two short branches);
* `COSO` at `$9F4`, `$20D2` and `$33B8`, each with `TFMX` 32 bytes later;
* the file is at least `$3888` bytes long;
* the first `TFMX` in the file (the one `hippel.rs` `tfmx_version` finds, at `$A14`) lies inside a
  `COSO` block: it is not a TFMX song block, and the TFMX builds must not take the file. The hash
  matches none of `hippel.rs` `BUILDS`, so today the file is refused there.

### 2.3 A COSO song block

All offsets are from the block's start `S`. Only these fields are read:

| Field | Meaning |
|---|---|
| `S+$04` long | offset of the frequency-sequence offset table |
| `S+$08` long | offset of the volume-sequence offset table |
| `S+$0C` long | offset of the pattern offset table |
| `S+$10` long | offset of the position list |
| `S+$14` long | offset of the subsong table (song A only; song B's entry is in the code) |

`S+$18`, `S+$1C` and the `TFMX` header's counts are not read (in song A, `+$1C` is even smaller
than `+$18`). An offset table is an array of words; entry `n` points at `S + word`, the word
taken as **signed** (sign-extended to 32 bits).

* **Position list**: 12 bytes a position, 3 a voice (voice 0 first): pattern number, note
  transpose (added to the note), volume-sequence transpose (added to the instrument number). The
  third byte has no command meaning in this replayer (values `$80`..`$FF` are transposes too).
* **Subsong table** (song A): 6 bytes an entry: first position, last position, speed (words).
  Song A's entries: (0, 35, 6), (36, 83, 4), (84, 213, 4), (0, 0, 4).
* **Pattern**: a byte stream, section 7.2.
* **Volume sequence**: 5 header bytes then the envelope bytes (7.4): speed, frequency-sequence
  number, vibrato speed, vibrato depth, vibrato delay.
* **Frequency sequence**: a byte stream (7.3).

### 2.4 Sample table

The sample block's `+$18` long gives the table; entry `n` is at `table + 10 n`:

| Bytes | Meaning |
|---|---|
| 0-3 | start: offset from the sample bytes (`$3888`), a long |
| 4-5 | length in words (written to AUDxLEN as is) |
| 6-7 | loop start: byte offset from the sample's start |
| 8-9 | loop length in words (written to AUDxLEN as is; 0 and 1 too) |

The sample address written to Paula is `module + $3888 + start` (the reference: `$45E0 + $3888 +
start`), the loop address that + loop start.

## 3. Host interface (as the reference calls the replayer)

* At load: InitSound with `d0` = 1. To start subsong `k` > 1 the host then steps through the
  subsongs: for `j` = 2..`k`, EndSound (InitSound with `d0` = 0) and InitSound with `d0` = `j`,
  back to back (section 9.4 times them).
* Every 70940 cc, the tick (entry `$0002`). The first interrupt comes 70940 cc after the last
  InitSound call returns.
* The reference shows subsongs 1..4. (Measured: the Eagleplayer's count is the number of `COSO`
  tags in the file plus one - removing one tag gives 1..3, appending one gives 1..5; it is not
  read from any header.) Subsong number = `d0`.
* The Eagleplayer patches the replayer's Paula accesses into calls of its own (`ENPP_*`); this
  changes only the timing (section 9) - every value and the order are the replayer's. The
  replayer's own writes of INTENA (`$0780`) and ADKCON (`$00FF`) and its CIA LED write are not
  done as such: the reference writes neither INTENA nor ADKCON, and turns the audio filter off
  (LED bit) through the host at each InitSound (not a logged register write).

## 4. Songs, subsongs, start and stop

### 4.1 InitSound(`d0`)

* `d0` = 0: **stop** (4.3).
* `d0` = 1: song B, first position 0, last position 204 (`$CC`), speed 3 (these three numbers are
  in the code; they equal song B's own first subsong entry).
* `d0` >= 2: song A, entry `d0 - 2` of its subsong table (first, last, speed). The reference
  reaches `d0` = 2, 3, 4 (song A's fourth entry, `d0` = 5, is never used).

Then, in this order:

1. The current song's tables are taken from its block (2.3); the sample table from the sample
   block (every InitSound reads it again; it does not change).
2. Filter off (host), then DMACON = `$000F` (all four voices' DMA off).
3. For each voice `v` = 0..3:
   * voice fields (6.1): volume sequence = the empty table (`$7EA`) at position 0, frequency
     sequence = the empty table at position 0; envelope counter 1, envelope speed 1, envelope
     sustain 0; frequency sustain 0, vibrato speed/depth/current/delay 0, the row's argument
     byte 0, note 0, info 0, volume 0, frequency value 0; row counter 0, row speed 0; the
     portamento accumulator's high word 0 (its low word is left; 7.6 clears the whole long at
     every note, so this is never seen); loop-offset word 0.
   * writes for voice `v`: AUDxLC = module + `$7EA` (the empty table), AUDxLEN = `$0010`,
     AUDxPER = 0, AUDxVOL = 0 (in this order; AUDxLC as one long write, high word first).
   * position: `pos0` = position list + 12 x first + 3 v; `posend` = position list + 12 x (last +
     1) + 3 v; position offset = 12; the first position's three bytes are read now: the
     pattern pointer = pattern table entry (byte 0), note transpose = byte 1, volume transpose =
     byte 2.
4. Song speed counter = 1, song speed = the subsong's speed, running.

So the first tick reads rows (the counter runs out at once) and every voice's first row byte is
the first byte of its first pattern.

### 4.2 Not reset by InitSound

The voice fields InitSound leaves alone are the loop address and loop length (written every
tick, 7.7) and the vibrato flags; the file's values count (6.2).

### 4.3 Stop (EndSound)

DMACON = `$000F`, then for each voice 0..3 AUDxPER = 0 and AUDxVOL = 0 (one long write per voice,
period word first). The player is marked stopped: a tick while stopped does nothing (no writes)
until the next InitSound with `d0` >= 1.

### 4.4 Unused parts

The code at `$0004` (an effects tick), `$01F8` (start an effect on a voice) and `$0290` (set the
effects up, with the sample block's own frequency and volume sequences) is never called by the
reference. The four "effect active" flags at `$094A` stay 0.

## 5. Song end and length

There is none: when a voice's position reaches `posend` it wraps to `pos0` (7.2); the voices
loop the subsong for ever. The reference reports no song end.

## 6. Voice record

### 6.1 Fields

The record is `$48` bytes (voice `v` at `$7F2 + $48 v`). The fields an implementation needs
(offsets in the record, for reading the saved values):

| Field | Off | Meaning |
|---|---|---|
| pos0 | `$00` long | the voice's column of the subsong's first position |
| posoff | `$04` long | offset of the next position from pos0 (steps of 12) |
| note | `$08` | the row's note byte (bit 7 kept) |
| info | `$09` | the row's info byte |
| vseq | `$0A` long | volume sequence (envelope part, past the 5 header bytes) |
| vpos | `$0E` long | position in it (signed; can be below 0 after an E0, 7.4) |
| fseq | `$12` long | frequency sequence |
| vtr | `$16` | volume-sequence transpose (position byte 2) |
| vcnt / vspd | `$17` / `$18` | envelope counter and speed |
| vsus | `$19` | envelope sustain counter (E8) |
| fsus | `$1A` | frequency sustain counter (E8) |
| vibspd / vibdep | `$1B` / `$1C` | vibrato speed and depth |
| vibcur | `$1D` | vibrato's current offset |
| vibdel | `$1E` | vibrato delay |
| arg | `$1F` | the row's optional third byte (portamento speed / frequency-sequence number) |
| voice number | `$20` | DMA bit number (0..3) |
| patp | `$22` long | pattern pointer |
| rowcnt / rowspd | `$26` / `$27` | row counter and row speed |
| fval | `$2B` | frequency-sequence value (transpose, or fixed note with bit 7) |
| ttr | `$2C` | note transpose (position byte 1) |
| volume | `$2D` | current volume |
| vibflag | `$2E` | vibrato flags: bit 5 direction (set = up), bit 0 toggles every tick, bit 7 (see 7.5) |
| fpos | `$30` long | position in the frequency sequence |
| posend | `$34` long | the voice's column one position past the subsong's last |
| porta | `$38` long | portamento accumulator |
| Paula register base | `$3C` long | `$DFF0A0 + 16 v` |
| loop | `$40` word, `$42` word, `$44` long | the last sample's loop start offset (unused afterwards), loop length (words), loop address |

### 6.2 Values saved in the file

The file was saved from the game's memory; InitSound does not reset these (4.2), so they are
read from the file:

| Voice | vibflag (`+$2E`) | loop length (`+$42`) | loop address (`+$44`) |
|---|---|---|---|
| 0 | `$41` | `$0010` | `$000593E6` |
| 1 | `$61` | `$0010` | `$00059426` |
| 2 | `$61` | `$0010` | `$00059446` |
| 3 | `$41` | `$0010` | `$000593A6` |

The loop addresses are the game's absolute addresses, **not** relocated: the reference writes
`$00059426` as is (e.g. voice 1's loop on the first ticks of subsong 1, before its first sample
command). A render of those ticks plays whatever the machine has at that address (a volume of 1,
from the empty table, is set on those voices), which the write log does not show.

## 7. The tick

### 7.1 Order

If stopped: nothing. Otherwise:

1. Song speed counter - 1; if it reaches 0: reload it with the song speed and run the rows of
   voices 0, 1, 2, 3 (7.2).
2. DMA mask = 0.
3. For each voice 0..3: its effects (frequency sequence 7.3, envelope 7.4, period 7.5,
   portamento 7.6), then AUDxPER = the period, AUDxVOL = the volume (two word writes).
4. DMACON = `$8000 | mask` (also when the mask is 0: then `$8000`).
5. For each voice 0..3: AUDxLC = loop address (one long write), AUDxLEN = loop length.

DMA offs come during the rows (a new note) and during the effects (E2). Only E2 adds a voice to
the mask; a new note's DMA off does not (its DMA stays off until an E2 - normally the first
command of the new frequency sequence, in the same tick's effects).

### 7.2 Rows (per voice)

Row counter - 1 (a byte). If the result is >= 0 (as a signed byte): done. Otherwise row counter
= row speed, and the pattern bytes are read from patp:

* `$FF`: next position: `p` = pos0 + posoff; if `p` = posend, posoff = 0 and `p` = pos0. Note
  transpose = `p[1]`, volume transpose = `p[2]`, patp = pattern table entry `p[0]`, posoff + 12;
  go on reading from the new patp (the row counter is not changed again).
* `$FE n`: row speed = row counter = `n`; go on reading.
* `$FD n`: row speed = row counter = `n`; patp past `n`; done (no note this row).
* any other byte `c`: a row. note = `c`, info = the next byte; if `info & $E0` != 0 the byte after
  is arg (else arg keeps its value). patp past the row; porta = 0 (the whole long).
  * If `c` has bit 7 set: done (a tie: the note changes, nothing restarts).
  * Else: DMACON = the voice's bit (DMA off, written now). Instrument `i` = `(info & $1F) + vtr`
    (8-bit, wrapping); its volume sequence `V` = volume table entry `i`. vpos = 0; vcnt = vspd =
    `V[0]`; vibspd = `V[2]`; vibflag = `$40`; vibdep = vibcur = `V[3]`; vibdel = `V[4]`; vseq =
    `V + 5`; frequency-sequence number = `V[1]`, or arg when `info & $40`; fseq = frequency table
    entry of that number (0..255); fpos = 0; fsus = 0; vsus = 0. Done.

### 7.3 Frequency sequence (per voice, per tick)

1. If fsus != 0: fsus - 1; skip to the envelope (7.4).
2. Read `c` = fseq[fpos] and dispatch:
   * `$E1`: hold (nothing read; skip to 7.4).
   * `$E0 x`: fpos = `x & $3F`; `c` = fseq[fpos]; then dispatch `c` **only** among E2, E4, E7,
     E8, E3 below (an `$E0` or `$E1` there is taken as a value).
   * then:
   * `$E2 s`: sample `s` with a restart: mask |= the voice's bit; DMACON = the voice's bit (DMA
     off, written now); AUDxLC = sample address, AUDxLEN = sample length (written now); loop
     address = sample address + loop start, loop length = the entry's; vpos = 0, vcnt = 1 (the
     envelope restarts next step; vsus is not cleared); fpos + 2; then the value step.
   * `$E4 s`: the same writes of AUDxLC and AUDxLEN and the same loop, without the DMA off, the
     mask and the envelope restart; fpos + 2; then the value step.
   * `$E7 t`: fseq = frequency table entry `t`, fpos = 0, and go back to step 2's dispatch from
     the `$E1` test (on the new sequence's first byte), in the same tick.
   * `$E8 n`: fsus = `n`, fpos + 2, and go back to step 1 (so `n` = 0 reads on at once; `n` > 0
     holds this tick and `n - 1` more).
   * `$E3 a b`: vibspd = `a`, vibdep = `b`, fpos + 3; then the value step.
   * anything else (also `$E5`, `$E6`, `$E9`..`$FF`): the value step.
   * **value step**: fval = fseq[fpos], fpos + 1. (After E2, E4 and E3 the byte that follows the
     command is taken as the value whatever it is.)

### 7.4 Envelope (volume sequence)

1. If vsus != 0: vsus - 1; done.
2. vcnt - 1 (byte); if not 0: done. Else vcnt = vspd and read `c` = vseq[vpos]:
   * `$E8 n`: vpos + 2, vsus = `n`, and go back to step 1 (`n` = 0: step 2 runs again, so vcnt is
     decremented once more in the same tick).
   * `$E1`: hold.
   * `$E0 x`: vpos = `(x & $3F) - 5` (`x` counts from the sequence's header), read again (all
     three commands are recognised there).
   * anything else (also `$E2`..`$E7`, `$E9`..`$FF`): volume = `c`, vpos + 1.

The volume is written as it is (no scaling, no clamp; the reference writes e.g. `$00E4` when a
sequence has `$E4` as a volume byte).

### 7.5 Period and vibrato

* Note index `n`: if fval has bit 7 set, `n` = fval & `$7F` (a fixed note); else `n` = (fval +
  note + ttr) & `$7F` (8-bit sum, then 7 bits; note's own bit 7 is lost in the mask).
* Period `P` = the word at module `$94E + 2n` (section 8; `n` up to 127).
* Vibrato: if vibdel != 0: vibdel - 1 and no vibrato this tick. Else:
  * `d4` = (2 x vibdep) mod 256; `d1` = vibcur.
  * Unless vibflag has bit 7 **and** bit 0 set (the vibrato then holds `d1` this tick):
    if bit 5 is clear (going down): `d1` = `d1` - vibspd; if that is below 0, bit 5 is set and
    `d1` = 0. If bit 5 is set (going up): `d1` = (`d1` + vibspd) mod 256; if `d1` >= `d4`, bit
    5 is cleared and `d1` = `d4`. vibcur = `d1`.
  * offset `o` = `d1` - (`d4` >> 1) (a signed number, -255..255).
  * scaling: `k` = 2n + 160; while `k` < 256: `o` = 2 `o`, `k` = `k` + 24. (Notes 48 and up: no
    doubling; 36..47: one; 24..35: two; 12..23: three; 0..11: four.)
  * `P` = (`P` + `o`) mod 65536.
* vibflag = vibflag XOR 1 (every tick, also during the delay).

Bit 7 of vibflag is never set by the replayer (a note sets `$40`); only a saved value can have
it, until the voice's first note.

### 7.6 Portamento

If info & `$20`: with `b` = arg: if `b` < `$80`: porta = porta + `b` x 2048 (32-bit), `P` = `P` -
(porta >> 16); else porta = porta + (256 - `b`) x 2048, `P` = `P` + (porta >> 16) (16-bit
wrap). porta is cleared at every row (7.2), ties too.

### 7.7 Writes

The period `P` and the volume of 7.4 go out as AUDxPER, AUDxVOL after the voice's effects; the
DMA on and the loops of 7.1 steps 4-5 after the last voice. Lengths are written as they are in
the sample table (no 1 -> 0 conversion).

## 8. Period table

The 83 words at `$94E` (decimal):

```
 0: 1712 1616 1524 1440 1356 1280 1208 1140 1076 1016  960  906
12:  856  808  762  720  678  640  604  570  538  508  480  453
24:  428  404  381  360  339  320  302  285  269  254  240  226
36:  214  202  190  180  170  160  151  143  135  127  120  113
48:  113 x 12
60: 3424 3232 3048 2880 2712 2560 2416 2280 2152 2032 1920 1812
72:  113  113  113  113  113  113  113  113 10344  113  113
```

Indices 83..127 read the next file bytes (song A's `COSO` header and the start of its data):
83: `$434F`, 84: `$534F`, 85: 0, 86: `$0040`, 87: 0, 88: `$01FC`, 89: 0, 90: `$03C4`, 91: 0,
92: `$0CBE`, 93: 0, 94: `$16C6`, 95: 0, 96: `$16DE`, 97: 0, 98: `$1261`, 99: `$5446`, 100:
`$4D58`, 101: `$0010`, ... - i.e. `period(n)` = word at `$94E + 2n` for every `n` in 0..127. An
implementation should read the module bytes, not a copy of the table.

## 9. Timing

### 9.1 The clock model

Every Paula write of the reference is a host call. The clock between two host calls advances by
`floor(c / 2)` cc, where `c` is the sum of the 68000 cycles run since the previous host call (or
since the interrupt call's start); the odd cycle, if any, is dropped and the count restarts at
each host call. A long write (AUDxLC) is one host call: both words at the same clock. The DMA-on
call (DMACON with bit 15) then moves the clock to the start of the 10th raster line after the
current one: `clock = (floor(clock / 227) + 10) x 227`, and the count restarts there.

An implementation keeps a cycle accumulator: add each path's cost (9.2, 9.3) as the algorithm
takes it; at a write, wait `floor(acc / 2)` cc, write, set acc = 0.

### 9.2 Code-path costs (cycles)

The costs below are the reference's (its 68000 core charges each instruction a fixed number of
cycles, conditional branches included, so they do not depend on the branch's direction). A path
is charged where the algorithm takes it; the names say what they cover.

Tick:

| Path | Cycles |
|---|---|
| interrupt to the speed counter's test (includes the host's 88) | 308 |
| the counter ran out (reload, before the rows) | 16 |
| per voice, before its rows and before its effects (a call) | 26 each time |
| after the rows (or the counter test without rows), before voice 0's effects | 20 |

Rows (per voice, after the call):

| Path | Cycles |
|---|---|
| row counter test | 25 |
| ... counter not run out (return) | 16 |
| ... run out: reload, first byte read | 36 + 25 |
| `$FF`: next position | 61 (+ 28 when it wraps) + 180 + 25 (the next byte's read) |
| `$FE n` | 59 + 25 (next read) |
| `$FD n` (return included) | 98 |
| a row: note and info | 83, + 16 with the third byte, + 47 |
| ... tie (return) | 16 |
| ... new note: to its DMA-off write | 36 + 220 (host) |
| ... after the DMA off: the instrument | 12 (host) + 263, + 12 when info bit 6 takes arg, + 126 (return included) |

Effects (per voice, after the call): 4, then:

| Path | Cycles |
|---|---|
| frequency sustain test | 21 |
| ... sustain counting down | 26 (then the envelope) |
| ... read the byte | 34 |
| `$E1` test (every dispatch) | 23 (`$E1`: then the envelope) |
| `$E0` test | 21 |
| ... `$E0` taken | 68 |
| `$E2` test | 21 |
| ... `$E2`: to its DMA-off write | 48 + 208 (host) |
| ... after the DMA off, to the AUDxLC write | 164 + 234 (host) |
| ... AUDxLC to AUDxLEN (host) | 202 |
| ... after AUDxLEN | 32 (host) + 138, then the value step |
| `$E4` test (when not `$E2`) | 21 |
| ... `$E4`: to the AUDxLC write | 164 + 234 (host) |
| ... AUDxLC to AUDxLEN (host) | 202 |
| ... after AUDxLEN | 32 (host) + 98, then the value step |
| `$E7` test | 21 |
| ... `$E7`: new sequence, back to the `$E1` test | 112 |
| `$E8` test | 21 |
| ... `$E8`: back to the sustain test | 54 |
| `$E3` test | 21 |
| ... `$E3` | 68 |
| value step | 74 |

Envelope:

| Path | Cycles |
|---|---|
| sustain test | 21 |
| ... sustain counting down | 26 (done) |
| counter decrement and test | 25 (done when not 0) |
| reload | 20 |
| read the byte, `$E8` test | 61 |
| ... `$E8`: back to the sustain test | 54 |
| `$E1` test | 17 (`$E1`: done) |
| `$E0` test | 17 |
| ... `$E0`: back to the read | 62 |
| a volume | 40 |

Period, vibrato, portamento:

| Path | Cycles |
|---|---|
| start | 21, + 24 when fval has bit 7 clear (the sum), + 73 |
| vibrato delay counting down | 26 |
| vibrato | 45, + 19 when vibflag bit 7 is set (then the hold test) |
| ... direction test (unless held) | 19 |
| ... down | 21, + 26 when it turns |
| ... up | 25, + 18 when it turns |
| ... store (unless held) | 12 |
| ... offset | 21, + 8 when it is negative |
| ... scaling | 17, + 21 per doubling |
| ... add | 4 |
| portamento test | 47 |
| ... portamento | 35, + 78 (up) or 72 (down) |
| end of the voice's effects | 32 |

Writes (per voice, after its effects; host costs marked):

| Path | Cycles |
|---|---|
| to AUDxPER | 20 + 230 (host) |
| AUDxPER to AUDxVOL (host) | 166 |
| after AUDxVOL, before the next voice's call | 20 (host) + 4 |
| after voice 3: to the DMACON write | 160 + 248 (host) |
| after the DMA-on line wait: to voice 0's AUDxLC | 186 (host) |
| AUDxLC to AUDxLEN: voice 0 / voices 1-3 (host) | 210 / 198 |
| AUDxLEN to the next voice's AUDxLC (host) | 182 |

Example: the PER/VOL pair is always 83 cc apart (166 cycles); a voice's AUDxLC and AUDxLEN in the
loops 105 cc (voice 0) or 99 cc apart, a loop's AUDxLEN to the next AUDxLC 91 cc; voice 0's
loop comes 93 cc after the line boundary.

### 9.3 Start-up times (cc)

From the InitSound call (clock `C`):

| Write | After |
|---|---|
| DMACON `$000F` | `C` + 535 (`d0` = 1), `C` + 579 (`d0` >= 2) |
| voice 0 AUDxLC | 434 after the DMACON |
| AUDxLEN, AUDxPER, AUDxVOL | 99, 91, 81 after the write before |
| voices 1-3 AUDxLC | 562 after the previous voice's AUDxVOL |
| return of InitSound | 406 after voice 3's AUDxVOL |

EndSound (called at the previous call's return, clock `E`): DMACON `$000F` at `E` + 275; voice
0's AUDxPER 97 later; each AUDxVOL 81 after its AUDxPER; voices 1-3's AUDxPER 89 after the
previous AUDxVOL; return 192 after voice 3's AUDxVOL. The next InitSound is called at that
return.

So InitSound returns `C` + 4145 (`d0` = 1) or `C` + 4189 (`d0` >= 2), EndSound takes 1155 cc,
and each further subsong step adds 5344 cc.

In the reference run of this file, `C` = 428474 cc from the 68000 machine's start (the
DMACON `$000F` at 429009). That time is the Eagleplayer's load and check, which depends on the
file (measured: +14 cc a byte when zero bytes are appended or the file is cut, and the synthetic
module of section 11, 14496 bytes, has `C` = 268916), so it is a constant of this file. The
first interrupt is 70940 cc after the last InitSound returns (subsong 1: 432619 + 70940 =
503559), the next every 70940 cc.

### 9.4 Check

The model above (values, order, every write's clock) reproduces the reference exactly, absolute
colour clock included, for subsongs 1-4 over 180 s each (8990 ticks, 685684 write intervals),
for two modified copies of the file whose song B exercises every path the songs do not take
(E7, E0 into each command and into values, E8 0 in both sequences, volume bytes `$E4`, info
bit 6, both portamentos, ties, notes past the table, a vibrato depth of `$90`, vibflag bit 7)
over 60 s each, and for the synthetic module over 10 s (section 11).

## 10. Differences from the existing code (`src/replay/hippel.rs`)

### 10.1 From `Coso` (the Hippel-COSO Eagleplayer's replayer)

Reusable: the COSO offset tables (word offsets from the block start, signed), the pattern byte
stream (`$FF`/`$FE`/`$FD`/row with an optional third byte when `info & $E0`), the 10-byte sample
entries, the volume-sequence header, the instrument number `(info & $1F) + transpose` (8-bit),
the frequency table from arg when `info & $40`, the envelope's E0 (`x - 5`) and E8 handling, the
fixed-note bit 7, the per-voice position columns. Different:

1. **Container**: the replayer code comes first; two song blocks and a sample block; the sample
   table from the sample block's `+$18`; the sample bytes at the fixed `$3888` (not `+$1C`, no
   `.samp` file); song B's start, end and speed are constants.
2. **Order**: rows first (when the speed counter runs out), then each voice's effects with its
   PER and VOL; `Coso` runs the effects, writes, then reads the rows.
3. **Writes**: PER then VOL per voice 0..3 right after its effects; then every tick DMACON
   `$8000 | mask` (even with no bits) and all four voices' AUDxLC/AUDxLEN loops; `Coso` writes
   VOL/PER/LEN/LC after a DMA on only, else VOL/PER of voices 3..0. E2 writes no AUDxPER = 0.
   Lengths are raw (`Coso`'s `len_word` 1 -> 0 does not apply).
4. **A new note turns its voice's DMA off** at once (DMACON = its bit, during the rows).
5. **No position commands**: position byte 2 is always the volume transpose (no fade, no stop,
   no speed command), no song end flag; no volume fade, variation or random generator: the
   volume is the envelope byte as it is.
6. **No `$80` test** on the volume sequence's frequency-sequence number.
7. **Frequency sequence**: only E0, E1, E2, E3, E4, E7, E8. After E2, E3, E4 the next byte is the
   value at once (no further commands that tick); E0 masks its target with `$3F` and then only
   E2/E4/E7/E8/E3 are recognised (E0/E1 there are values); E4 writes the sample's start with its
   length (not the loop); E7 switches to another frequency sequence (not a sample); E5, E6,
   E9-EA and `$EB`-`$FF` are values.
8. **Envelope**: only E0, E1, E8 are commands; `$E2`-`$E7` and `$E9`-`$FF` are volumes.
9. **Vibrato**: the early one (scale doubling, 7.5), with 8-bit sums; `Coso`'s is `(d * per) >>
   10`. **Portamento**: accumulator += arg x 2048, period -/+ accumulator >> 16; `Coso`'s is
   proportional to the period.
10. **Period table**: the module's own at `$94E`, read on into the file past index 82.
11. **InitSound**: each voice on the empty table (AUDxLC = `$7EA`, AUDxLEN = `$10`, PER 0, VOL
    0), row speed and counter 0 (not kept), the saved loops and vibrato flags of 6.2.
12. **Subsongs**: 1 = song B, 2..4 = song A's entries 0..2; `Coso` numbers the table's entries.
13. **Timing**: 9 (nothing of `Coso`'s `At::cost` applies; the DMA-on wait is the same 10-line
    rule).

### 10.2 From `Tfmx` with the early builds (`EARLY`: `$962` etc.)

The effects are those of `EARLY` - `mute` (a new note turns its DMA off), E7 switches the
frequency sequence (`e7: 1`), no E5/E6/E9, the older vibrato (`vib: 1`), `effloop: false` (after
E2/E3/E4 the next byte is the value), the portamento, E4 without a restart - and the speed
counter starts at 1 (`counter_from_speed: false`). Different:

1. **Data**: COSO (offset tables, variable-length patterns ended by `$FF`, `$FE`/`$FD` row speeds,
   10-byte sample entries, sample bytes at `$3888`), not TFMX (64-byte tables, fixed-size tracks
   with a note of 1 or the track's end moving on, 30-byte sample entries). There is no "info of
   the row before": info bit 6 takes the row's own third byte (as `freq_prev` does with
   `prev_info` in the middle builds; `EARLY` has `freq_prev: false`).
2. **Write order**: as `MIDDLE` (`split: false`, `dma_always: true`, all four loops every tick),
   with no line waits (`waits: 0`) and the host's 10-line DMA wait after the DMA on.
3. **Frequency sequence**: after E0 only E2/E4/E7/E8/E3 are tested (`Tfmx` re-reads from the
   top); `$E1` stops, but `$EA`-`$FF` are values (`Tfmx` stops on them).
4. **Envelope**: only E0, E1, E8 are commands (`Tfmx` stops on `$E1`-`$FF`).
5. **Vibrato bytes**: `2 x depth` and `current + speed` wrap at 256 and the limit test is
   unsigned (`Tfmx` uses wide integers): equal for depth < 128 and current + speed < 256.
6. **InitSound**: the empty table as the sample (`init_loc: Some(0)`, length `$10`), positions
   read at once (posoff 12), stop and subsong steps as in 4 and 9.3.

## 11. Test vectors

Reference runs and tools are in
`/media/lolo/kingston2tb/claude-scratch/deliplayerrust/fixround/astaroth/spec/` (README.md
there lists them):

* `vectors/astaroth_sub{1,2,3,4}_180s.ref.log`: the reference's `DELI_WRITE_LOG` for each subsong
  (180 s). `vectors/astaroth_sub{N}_100ticks.txt`: the start-up and the first 100 ticks, one write
  a line (`tick register value clock clock-minus-interrupt`).
* `vectors/synth.hip`, `vectors/synth_10s.ref.log`, `vectors/synth_sub1_100ticks.txt`: the
  synthetic module below and its reference run.

The first tick of subsong 1 (interrupt at 503559; cc after it):

```
DMACON 0001 @453   DMACON 0004 @1049  DMACON 0008 @1540        rows: notes on voices 0, 2, 3
DMACON 0001 @1953  AUD0LC 00007E88 @2152  AUD0LEN 0448 @2253   voice 0: E2
AUD0PER 00D6 @2700  AUD0VOL 002F @2783
AUD1PER 0071 @3353  AUD1VOL 0001 @3436                         voice 1: still the empty table
DMACON 0004 @3651  AUD2LC 00007E88 @3850  AUD2LEN 0448 @3951  AUD2PER 00D6 @4398  AUD2VOL 0018 @4481
DMACON 0008 ...    (voice 3 likewise)                          then DMACON 800D,
AUD0LC 00007E88 AUD0LEN 0001, AUD1LC 00059426 AUD1LEN 0010 (saved loop), AUD2.., AUD3..
```

### 11.1 Synthetic module

A file of `$38A0` bytes, all zero except (offsets in the file):

* `$0000`: `60 1E`, `$0002`: the ASCII marker `SYNTHAST` in place of the code (for a unit test's
  detection, as `hippel.rs`' other synthetic modules do).
* `$07EA`: `01 00 00 00 00 00 00 E1`. The voice records (`$7F2`-`$911`) stay zero: vibflag 0,
  loop length 0, loop address 0.
* `$094E`: the 83 periods of section 8 (indices past 82 read zeros here).
* Song B at `$20D2`: `COSO`, `+$04` = `$40`, `+$08` = `$44`, `+$0C` = `$48`, `+$10` = `$7A`,
  `+$14` = `$A16`, `+$18` = `+$1C` = 0; `+$20` = `TFMX` and the words 1, 1, 1, `$CC`, `$40`, 0, 0,
  2. Then at `+$40`: frequency table `004C 0053`, volume table `0057 0063`, pattern table `006B
  0075`, and the data:
  * F0 (`+$4C`): `E2 00 00 0C 00 E0 03`; F1 (`+$53`): `E2 01 00 E1`;
  * V0 (`+$57`): `01 00 00 00 00 3F 30 20 E8 04 10 E1`; V1 (`+$63`): `02 01 02 03 01 28 E0 05`;
  * P0 (`+$6B`): `18 00 FE 01 1B 00 1F 20 03 FF`; P1 (`+$75`): `FD 05 24 01 FF`;
  * the position list at `+$7A`: 205 identical rows `00 00 00 01 00 00 01 05 00 01 F4 00`
    (song B always plays positions 0..204); a dummy subsong entry `0000 00CC 0003` after them.
* Sample block at `$33B8`: `COSO`, `+$04` = `+$08` = `$40`, `+$18` = `$386`, `+$20` = `TFMX`, words
  0, 0, 0, 0, `$40`, 0, 0, 2.
* Sample table at `$373E`: `00000000 0008 0000 0008` (sample 0: 16 bytes at `$3888`, loop the
  whole) and `00000010 0004 0004 0002` (sample 1: 8 bytes at `$3898`, loop from its 4th byte, 2
  words).
* Sample bytes at `$3888`: `40` x 8, `C0` x 8, then `00 30 60 30 00 D0 A0 D0`.

`scripts/mksynth.py` there builds it. The reference cannot run the marker file; its reference run
(`exp/synth_refrun.hip`) is the same file with the real module's bytes at `$0000`-`$07F1` and
`$0912`-`$094D` (code and the replayer's own constants). Subsong 1, module at `$45E0`, InitSound
called at 268916; the first ticks (interrupt `I` = 273061 + 70940 k):

```
start-up: DMACON 000F @269451; per voice: LC 00004DCA, LEN 0010, PER 0, VOL 0 (as 9.3)
tick 1 (I = 344001): DMACON 0001 @+411 (row: P0 note $18, V0)  DMACON 0001 @+1139 (F0: E2 00)
   AUD0LC 00007E68 @+1338  AUD0LEN 0008 @+1439  AUD0PER 01AC @+1975  AUD0VOL 003F @+2058
   AUD1PER 0650 AUD1VOL 0001 | AUD2PER 04B8 AUD2VOL 0001 | AUD3PER 0000 AUD3VOL 0001
   DMACON 8001; loops: AUD0 00007E68/0008, AUD1-3 00000000/0000
tick 2: PER/VOL 00D6/0030, 06B0/0000, 0500/0000, 0000/0000; DMACON 8000; the same loops
tick 3: 01AC/0020 ...   tick 4: DMACON 0001 (row: note $1B), DMACON 0001 (E2), AUD0 LC/LEN, 0168/003F ...
```

(Voice 0's sample address `$7E68` = `$45E0 + $3888`. Voice 3's note is `1 + 0 + $F4` & `$7F` =
117, past the table: period 0 here.) The full expected writes, clocks included, are in
`vectors/synth_10s.ref.log`; times relative to InitSound's call hold for any load clock.

## 12. Status

Verified by experiment (reference runs compared write by write: value, order, colour clock):
everything in sections 4, 7, 8 and 9 over the runs listed in 9.4, including every code path of
the tick (the four subsongs take all but three - E7, info bit 6, vibflag bit 7 - which the
modified copies take). The cycle costs of the module's code are the reference core's per
instruction costs; the host's costs were fitted (integer, exact on all intervals).

Inferred, not run: InitSound with `d0` = 5 (song A's fourth entry; the reference never asks for
it); the sound-effect entry points (never called); a volume-sequence `E0` with a target below 5
(vpos negative: reads the sequence's header bytes; follows from 7.4).

Open questions:

* The load time `C` for files other than this one (the Eagleplayer's check scales with the file
  and its contents); only this file and the synthetic one were measured.
* Whether other modules carry this replayer build (none in the corpus known to the project; the
  two Astaroth `sog.*` files use other replayers).
* What a render plays from the saved loop addresses (6.2) before a voice's first sample: memory
  outside the module, as on the original machine.
