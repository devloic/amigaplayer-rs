# Special FX ST (`doda.*`) player: behaviour spec

*Sanitised for publication: behaviour only.*

Scope: the Eagleplayer "Special FX ST" (Wanted Team adaptation, version string "V1.3 (5 May 2013)", as
shipped with UADE), prefix `DODA.`. It plays the Atari ST music of Matthew Cannon and Jonathan Dunn (Special
FX, 1989-92, for example Ocean's "The Addams Family"). It is a different format from the Amiga "Special FX"
(`jd.*`) player.

A DODA file is an Atari ST memory image: the game's 68000 sound driver followed by its music tables. Unlike
the YM-2149/MYST player, this player does **not** run the module's code. It has its own copy of the replay,
finds the driver's music tables inside the file by searching for fixed byte patterns, interprets them, and
feeds the resulting YM-2149 register values to a small built-in "YM2149 emulation" that drives Paula with
three tiny looping samples (a square pulse, a noise block and a silent word).

The player has two emulation types, chosen by its configuration file: "Mad Max" (type 0, the default) and
"meynaf" (type 1). Only type 0 is specified here: the reference host never loads a configuration, so type 0
is what plays (see README).

This text was derived from write logs, beam-read traces and per-instruction cycle histograms of the
reference 68k player run through the host with DELI_WRITE_LOG, and checked by a private model written from
this text (section 8). Cycle counts are the host's static costs per instruction (a branch costs the same
taken or not). A write or read is stamped `C + floor(t / 2)` colour clocks (cc), where `C` is the clock at
which the host calls the tick routine and `t` counts CPU cycles since then, including the whole
instruction that does the access.

## 1. Detection

A file is accepted when all of these hold (offsets from the file start, big-endian):

1. size > 2300 bytes;
2. the first longword is 0 or 0x00000101, or the first word is 0x0101 or 0x6000;
3. at some even offset `m` below 140 the longword is 0x00090800, and the longwords at `m+74`, `m+148`,
   `m+222`, `m+296`, `m+370` are 0x01120900, 0x02240A00, 0x00090800, 0x01120900, 0x02240A00 (these are the
   tails of the driver's six 74-byte voice records; only the first even `m` with 0x00090800 is tried);
4. the longword 0x0EF80E10 (the first two entries of the driver's period table) occurs at some even offset
   `p >= 2000`. The search checks offset 2000 first and then each following even offset while it is below
   `size - 4`.

The player reads nothing else to detect. No other UADE player uses the prefix.

## 2. Locating the tables (at load)

### 2.1 How the search works

The driver is located by fixed 16- and 32-bit values that occur in its code next to the references to its
tables. A table reference is a signed 16-bit displacement `D` stored at some offset `x`; the table is at
`x + D`. Below, "the displacement at `x`" means exactly that. Two of the references may instead be absolute
32-bit addresses (2.3).

A counter starts at the file size. Every forward step of 2 bytes in the searches below subtracts 2 from it,
the skips noted subtract 10. If it goes negative the load fails ("error in file"). The searches run one
after the other, each starting where the previous one stopped (position `q`):

1. From offset 0, find the first even offset `q` holding the word 0xE740. The **subsong table** is at the
   displacement at `q - 2`. The word at `q - 10` is a provisional subsong count. Continue at `q + 2`.
2. Walk forward. At each position: if the longword there is 0x101A234A, the **extra-pattern table** is at
   the displacement at `q + 8`; subtract 10 and skip 10 bytes (then continue walking). Else, if the longword
   is 0x4880D040, stop: the **pattern table** is at the displacement at `q - 2`. Else step 2. If 0x101A234A
   never occurs, the extra-pattern table keeps the value of the previous module (0 at first).
3. From there find the longword 0x08C70007 at `q`. Search backwards from `q`, word by word, for the first
   word 0x41FA (no counter, no bound); the **drum table** is at the displacement that follows that word.
4. From `q`, walk forward until a word 0x41F9 or 0x41FA. For 0x41F9 the **trigger table** is the absolute
   address in the longword that follows it, and the walk stops on the 0x41F9 word; for 0x41FA it is at the
   displacement that follows, and the walk stops on that displacement.
5. Find the longword 0x08870002 at `q`. If the word at `q + 4` is 0x41F9, the **arpeggio table** is the
   absolute address at `q + 6`, and this address is remembered as the image base `A`. Else it is at the
   displacement at `q + 6`.
6. Find the longword 0x08C70002 at `q`; subtract 10 and move to `q + 10`. If the word at `q + 8` is 0x41F9
   the **slide table** is the absolute address at `q + 10`, else at the displacement at `q + 10`.
7. Walk forward to the first word 0x41FA; the **volume-envelope table** is at the displacement that follows
   it. Continue at that displacement.
8. Walk forward checking longwords. 0x0EF80E10 ends the whole search (no further tables). 0x7000101A at
   `q`: the **slide-list table** used by command 0x8C is at the displacement at `q - 2`.
9. Then walk forward again: 0x0EF80E10 ends the search; the first word 0x41FA gives the digital-sample
   table (unused by this player; its offset from the file start becomes the module's "calc size" and "song
   size"). If that offset exceeds the file size the load fails ("module too short").
10. Only if an image base `A` was found in step 5: from the digital-sample displacement walk forward word
    by word. Each time the word is 0x41FA, the calc/song size becomes `(that position + 2) + displacement
    + 14`. Stop at the longword 0x00110010; let `E` = its position + 4 (in memory). The arpeggio, slide and
    trigger table addresses are then rebased: `address - A + E`. (The trigger table is rebased even if it
    was found by displacement.)

Subsong count: if `(pattern table - subsong table) / 8` (the subtraction must be non-negative) is not zero,
it replaces the provisional count. Subsongs are numbered 1..count.

### 2.2 Table formats

"Table" entries are 16-bit words, entry `n` at `table + 2n`, each a displacement relative to the entry's
own address. Indices are signed when they come from a signed byte (noted below).

| table | entry points at |
|---|---|
| subsong | 8 bytes per subsong: three words for voices A, B, C, each a displacement relative to that word to the voice's **sequence**, or 0 = voice not used. The fourth word is ignored |
| pattern, extra-pattern | a **pattern** (byte stream, 3.3) |
| drum | a list of 4-byte steps `mask, volume, period(word)`; a step with `mask & voice mask` = 0 ends it |
| trigger | `tick`, then a volume envelope (format below) that starts at the next byte |
| arpeggio | `speed, length`, then `length` signed semitone offsets |
| slide, slide-list | a list of word pairs `count, delta`. A negative `count` is followed by a word displacement (relative to itself) to the pair to continue with |
| volume envelope | byte pairs `count, volume`. `count` 0 = hold. A negative `count` is followed by a signed byte displacement (relative to itself) to the pair to continue with |

### 2.3 Module information

Subsong count as above, voice count 3 (minus unused voices for the current subsong), max voices 3,
"length" (3.6). There is no title or author text in the format.

## 3. The replay

### 3.1 State

Three voices A, B, C. Each has a fixed **voice mask** (its YM mixer bits: tone bit and noise bit) and a YM
volume register: A 0x09 / 8, B 0x12 / 9, C 0x24 / 10. Per voice:

- flags (16 bits): bit 15 = active; bit 0 = note kick; bit 1 = arpeggio; bit 2 = slide; bit 3 = volume
  envelope running; bit 4 = envelope trigger armed; bit 5 = this voice sets the YM noise period; bit 6 =
  fixed duration; bit 7 = drum running;
- pattern position, sequence start, sequence position, pattern repeat counter, transpose (byte);
- mixer bits `mix` (subset of the voice mask that is turned on), duration counter `dur` and fixed duration;
- note `n0` (after transpose), current note `n` (after kick), current period `per` (word), volume `vol`;
- kick offset, kick length, kick counter; drum position; trigger tick and trigger envelope;
- arpeggio start, position, length, remaining, speed, timer; slide start, position, counter (word),
  delta (word); envelope start, position, counter.

The YM shadow: bytes R0..R10 (R0/R1 period low/high of A, R2/R3 of B, R4/R5 of C, R6 noise period, R7
mixer with a set bit = off, R8..R10 volumes).

Period table (102 words, index = note, signed): 0xEF8, 0xE10, 0xD60, 0xC80, 0xBD8, 0xB28, 0xA88, 0x9F0,
0x960, 0x8E0, 0x858, 0x7E0, 0x77C, 0x708, 0x6B0, 0x640, 0x5EC, 0x594, 0x544, 0x4F8, 0x4B0, 0x470, 0x42C,
0x3F0, 0x3BE, 0x384, 0x358, 0x320, 0x2F6, 0x2CA, 0x2A2, 0x27C, 0x258, 0x238, 0x216, 0x1F8, 0x1DF, 0x1C2,
0x1AC, 0x190, 0x17B, 0x165, 0x151, 0x13E, 0x12C, 0x11C, 0x10B, 0xFC, 0xEF, 0xE1, 0xD6, 0xC8, 0xBD, 0xB2,
0xA8, 0x9F, 0x96, 0x8E, 0x85, 0x7E, 0x77, 0x70, 0x6B, 0x64, 0x5E, 0x59, 0x54, 0x4F, 0x4B, 0x47, 0x42,
0x3F, 0x3B, 0x38, 0x35, 0x32, 0x2F, 0x2C, 0x2A, 0x27, 0x25, 0x23, 0x21, 0x1F, 0x1D, 0x1C, 0x1A, 0x19,
0x17, 0x16, 0x15, 0x13, 0x12, 0x11, 0x10, 0xF, 0xE, 0xD, 0xC, 0xB, 0xA, 0x9. These are YM tone periods.
Indices outside 0..101 read the player's memory around the table: indices -128..-1 give the words of
`data/period_underflow.bin` (256 bytes, index -1 = its last word); indices 102..121 give the ten located
table addresses as 32-bit values, high word first, in the order arpeggio, slide, volume envelope, trigger,
drum, subsong, pattern, extra pattern, slide list, digital-sample table (absolute addresses in memory);
indices 122..127 give the words of `data/period_overflow_tail.bin`. (Both files are player code; the port reads
0 for those indices instead.)

### 3.2 Subsong start

All voice state is cleared (all fields 0) and the YM shadow bytes are cleared. The emulator state is reset
(4.1). The song-end bytes are set to (0xFF, 0x00, 0xFF, 0xFF). For each voice whose subsong word is not 0:
flags = 0x8000, `dur` = 1, repeat counter = 1, transpose = 0, and the sequence is read from its start:

- byte below 0x80: pattern number; the pattern is entry `n` of the pattern table. The sequence position
  is the next byte. Done.
- 0x80: the voice is not started (flags 0). Done.
- 0xFF, `x`: the pattern is entry `x` (signed) of the extra-pattern table; the sequence position is after
  `x`. Done.
- 0x81, `d`: continue reading at the address of `d` plus `d` (signed).
- 0x82, `r`: repeat counter = `r`. 0x83, `t`: transpose = `t`. Continue.
- other bytes 0x84..0xFE: ignored, continue.

A voice whose subsong word is 0 decrements the module's voice count.

### 3.3 The tick

The host calls the tick routine at 50 Hz (every 70 940 cc). In order:

1. If no voice is active (bit 15 clear in all three), the host's song-end function is called (and, when
   the length of 3.6 is above 10, the subsong is restarted as in 3.2; the host stops at song end, so this
   was not timed). The tick then continues normally.
2. The replay: R7 = 0xFF. For voice A, B, C in turn, if active, the voice routine (3.4) returns a period
   `P` (word) and a volume `V`. Then flags are stored, R(8+i) = `V`, R(2i) = low byte of `P`, R(2i+1) =
   high byte, and if flags bit 5 is set R6 = high byte of `P`. A voice that is not active leaves its
   registers as they were (its mixer bits stay off because R7 was reset).
3. R0..R10 are copied to the emulator's registers; the emulator (section 4) runs.

All Paula writes of a tick happen in step 3, in the same tick as the replay step that produced them.

### 3.4 Voice routine

`dur` is decremented (8 bits). If it became 0, events are read from the pattern (3.5). Then the effects:

**Drum (flags bit 7).** At the drum position: `m = voice mask & mask byte`. If `m` = 0, return `P` = 0, `V`
= 0 (R7 untouched, the position does not move). Else R7 = `(R7 | voice mask) XOR m`, `V` = the volume
byte, `P` = the period word, R6 = low byte of `P`; the position moves 4 bytes. Return.

**Normal.** R7 = `(R7 | voice mask) XOR mix`. `P` = `per`. Then, in this order:

- kick (bit 0): kick counter - 1 (8 bits). When it wraps from 0 to 0xFF: `n = n0`, `P` = period(`n0`).
- arpeggio (bit 1): timer - 1; when it reaches 0: take the next offset `o`; remaining - 1, and when that
  reaches 0 the position returns to the arpeggio start and remaining = length; timer = speed; `P` =
  period((`o + n`) & 0xFF, signed).
- slide (bit 2): counter - 1 (16 bits); when it reaches 0, read the next pair (following a negative count
  through its displacement): counter = count, delta = delta. Every tick with bit 2: `P = P + delta`.
- `per` = `P`.
- trigger (bit 4): if trigger tick = `dur`, the envelope position = trigger envelope, envelope counter = 1.
- volume envelope (bit 3): counter - 1; when it reaches 0 read a count byte: 0 = keep counter 0 and do not
  move (the next tick it wraps to 255, so the envelope holds for 256 ticks before the 0 is read again);
  negative = follow the displacement and read the count there (not checked again); then `vol` = the next
  byte and the position moves past it.
- return `P`, `V` = `vol`.

### 3.5 Pattern events

Bytes are read until an event that ends the step. "Duration" below means: if flags bit 6 is set, `dur` =
fixed duration, else `dur` = the next byte. After a duration the voice routine continues with the effects
(same tick).

- **0x00..0x7F, note.** `n0 = (byte + transpose) & 0xFF`. If bit 0 is set: `n = n0 + kick offset`, kick
  counter = kick length; else `n = n0`. `per` = period(`n`) (index signed). Arpeggio position = start,
  remaining = length, timer = 1; slide position = slide start, slide counter = 1; envelope position =
  envelope start, envelope counter = 1; `vol` = 0; set bit 3, clear bit 7. Duration.
- **0x80** stop: flags = 0, return `P` = 0, `V` = 0 at once (no effects).
- **0x81 `d`**: continue reading at the address of `d` plus `d` (signed).
- **0x82** end of pattern: repeat counter - 1. If not 0, the pattern number is the sequence byte just
  before the sequence position (signed; looked up in the *pattern* table even if it came from a 0xFF
  entry) and reading continues there. Else repeat counter = 1, transpose = 0, and the sequence is read as
  in 3.2 with these differences: a pattern number sets the pattern and continues reading events there;
  0x80 stops the voice (as event 0x80); 0xFF `x` continues in extra pattern `x`; 0x81 `d` jumps and runs
  the song-end test (3.6) for this voice.
- **0x83 / 0x84 / 0x85**: `mix` = voice mask & 0x07 (tone only) / & 0x38 (noise only) / & 0x3F (both);
  0x83 clears bit 5, 0x84 and 0x85 set it.
- **0x86**: tie (duration, the note goes on).
- **0x87**: rest: clear bit 3, `vol` = 0, duration.
- **0x88 `d`**: fixed duration = `d`, set bit 6. **0x89**: clear bit 6.
- **0x8A `o` `l`**: set bit 0, clear bit 1, kick offset = `o`, kick length = `l`. **0x8B**: clear bit 0.
- **0x8C `k`**: set bit 2, clear bit 1, slide start = entry `k` (unsigned) of the slide-list table.
- **0x8D `x y z`**: clear bit 7, skip the 3 bytes, then as 0x87.
- **0x8E, 0x8F**: ignored.
- **0x90..0x97**: drum position = drum entry `byte - 0x90`, set bit 7, duration.
- **0x98..0x9F**: `k = byte - 0x98`; 0 clears bit 4; else set bit 4, trigger tick and envelope from
  trigger entry `k`.
- **0xA0..0xBF**: `k = byte - 0xA0`; 0 clears bit 1; else set bit 1, clear bit 2, speed, length and start
  from arpeggio entry `k`, timer = 1.
- **0xC0..0xDF**: `k = byte - 0xC0`; 0 clears bit 2; else set bit 2, clear bit 1, slide start = slide
  entry `k`.
- **0xE0..0xFF**: envelope start = volume-envelope entry `byte - 0xE0`.

Every command that does not end the step continues with the next byte. Note: `mix` starts as 0, so a voice
is silent (both its mixer bits off) until a pattern uses 0x83..0x85.

### 3.6 Song end, length, position

- Song-end test (on a sequence jump, 0x81 inside 0x82): voice A clears byte 0, voice C byte 2, voice B
  byte 3 and also byte 2 when voice C is not active. When all four bytes are 0, they are set back to (0xFF,
  0x00, 0xFF, 0xFF) and the host's song-end function is called. (A song whose voice A is unused never
  ends this way.)
- When all three voices are inactive at the start of a tick, the host's song-end function is called (3.3).
- Length (module info, also the restart threshold): with one voice in use, 1. Else `start of B's
  sequence - start of A's sequence` (byte distance) if not negative; else 1 + the number of bytes from A's
  sequence start up to the first 0x80 or 0x81.
- Position: voice A's sequence position minus its sequence start.

## 4. The emulator (type 0, "Mad Max")

### 4.1 State and samples

Four records, one per Paula channel: voice A on channel 0, B on channel 3, C on channel 2, and the noise
mix on channel 1. Each record: `vol` (word), `per` (word), `mode` (0 silent, 1 pulse, 2 noise). Reset (3.2):
`vol` 0, `per` 0x100, `mode` 0, noise count 0, emulator registers R0..R6 = 0, R7 = 0x3F, R8..R10 = 0.

Three sample buffers in chip memory, at fixed offsets from a base `B` (the comparison with the reference
needs the same addresses): noise `B` (1024 bytes, played as 0x200 words), pulse `B + 0x4402` (bytes 0xB2,
0x4D), silent `B + 0x4404` (two zero bytes). (`B + 0x4400` holds 0x70, 0x90, the pulse of type 1.) The
noise bytes are generated once, when the player is first loaded: a 32-bit state `x` starts at 0x48495050.
For each byte: `y = 3677 * x` (32 bits); `z = x + 0xE90` (32 bits), then the low 16 bits of `z` are shifted
left by 4 (upper 16 bits kept); `x = ((z + y) with bit 31 cleared) - 1` (32 bits); the byte is bits 8..15
of `x`.

Volume table `VT[16]` = 0, 1, 2, 3, 4, 5, 7, 9, 12, 16, 21, 27, 34, 42, 52, 64.

### 4.2 Per tick

`en` = NOT R7 (a set bit = on). For voice A, B, C (records 0, 1, 2 in that order, channels 0, 3, 2), with
period `p = R(2i+1) << 8 | R(2i)`, volume `v = R(8+i) & 15`, tone bit `i`, noise bit `i + 3`:

1. `rec.vol` = `VT[v]` (low byte; the high byte stays 0).
2. Tone on: `rec.per` = `(((p & 0xFFF) * 0x38CC) >> 10) + 1` (16 bits). If noise is also on: noise count
   + 1 and the noise record's `vol` += `rec.vol`. If `mode` is not 1: `mode` = 1, **restart** the channel
   with the pulse sample, length 1. Then **write volume and period**.
3. Else noise on: the **noise path**: `rec.per` = `(R6 & 0x1F) * 16 + 0x8E`. If `mode` is not 2: `mode` = 2,
   restart with the noise sample, length 0x200. Write volume and period.
4. Else (both off): **disable**: `rec.vol` = 0, `rec.per` = 0x100, `mode` = 0, restart with the silent
   sample, length 1, then write volume and period. This happens **every tick** for a voice whose mixer
   bits are both off (inactive voices too).

Then the noise channel (record 3, channel 1), by noise count `k`:

- `k` = 0: if its `mode` is not 0, disable it (step 4); else nothing.
- `k` = 1, 2, 3: its `vol` is kept, halved (shift right 1), or divided by 3 (integer). Noise path (step 3)
  on record 3. Then its `vol` = 0 and `k` = 0.

**Write volume and period**: AUDxVOL = `(rec.vol * gain) & mask >> 6` with gain = the left volume for
channels 0 and 3, the right volume for channels 1 and 2 (each `balance * master volume >> 6`, 64 by
default), mask = 0xFFFF, or 0 for a channel turned off with the host's voice switches; then AUDxPER =
`rec.per`.

### 4.3 Restart of a channel (sample `S`, length `L`)

The channel's DMA bit `b` (1, 8, 4, 2 for records 0..3, i.e. channels 0, 3, 2, 1):

1. Wait for the start of a raster line, then until the horizontal position is at least 22 (4.4).
2. AUDxPER = 1.
3. `s` = DMACON & `b` (as read back). INTREQ = `s << 7` (clears the channel's request if it was playing),
   DMACON = `s` (stops it, or writes 0).
4. Poll INTREQ until all bits of `s << 7` are set (immediately if `s` = 0). This waits for the
   stopped channel to reach the end of the byte it is playing.
5. AUDxLC = `S` (both words at one stamp), AUDxLEN = `L`, AUDxPER = `rec.per`.
6. Wait for a line start and horizontal position 22 again.
7. `s` = (NOT DMACON) & `b`. INTREQ = `s << 7`, DMACON = `0x8000 | s`.
8. Poll INTREQ until all bits of `s << 7` are set (the host's Paula raises it at the first line start
   after DMA on).
9. Wait for a line start and horizontal position 22.
10. AUDxLC = `S`, AUDxLEN = `L` (the loop part, the same sample).

### 4.4 Beam and interrupt waits

The beam position read back is the line (low 8 bits) and the horizontal count `h` (0..226, in cc); a line
is 227 cc and lines start at multiples of 227 cc. INTREQ is the host Paula's interrupt request register,
brought up to date at each read (an implementation polls the host Paula at the same clocks). The polling
loops have fixed costs (section 6) so that the exit points are reproducible.

## 5. Other entry points

- End of a subsong (the host's end-sound call, also at subsong changes): DMACON = 0x000F, then AUD0..3 VOL
  = 0.
- Voice switches: for each disabled channel its volume register is written 0 and the channel mask becomes
  0 (4.2).
- Subsong start writes nothing to Paula.

## 6. Timing (CPU cycles)

`t` starts at 0 when the host calls the tick routine. Costs are cumulative; a write or read happens at the
end of the step that names it.

### 6.1 Tick and replay

| step | cycles |
|---|---|
| entry until the replay starts: voice A active / A inactive, B active / only C active / none active (includes the host's song-end function, measured once) | 271 / 292 / 313 / 430 |
| replay start | 20 |
| per voice: test (all three voices) | 27 |
| active voice: call | 34 |
| after the voice routine returns: store | 115, +16 if flags bit 5 |
| after voice C: copy to the emulator and return | 242 |
| from there to the emulator start | 39 |

Voice routine (3.4):

| step | cycles |
|---|---|
| `dur` decrement and test | 27 |
| each event byte read | 19 |
| note | 293, +32 with bit 0 |
| duration (after a note, drum, 0x86, 0x87, 0x8D) | 45, +8 when the byte is read |
| commands 0x80..0x8F: dispatch | 19, then 65 (0x8E/0x8F: 51 in place of 65, and nothing else) |
| 0x80 / 0x81 / 0x83 / 0x84 / 0x85 / 0x86 / 0x87 / 0x88 / 0x89 / 0x8A / 0x8B / 0x8C / 0x8D | 22 / 30 / 52 / 50 / 50 / 10 / 40 / 38 / 24 / 68 / 24 / 104 / 72 |
| 0x82 (see below) | |
| commands 0x90..0xFF: dispatch 19, then 0xE0+ 89; 0xC0+ 49; 0xA0+ 68; 0x98+ 87; 0x90+ 177 (complete) | |
| 0xC0/0xA0/0x98 groups, index 0 | +24 |
| index not 0: slide 96, arpeggio 144, trigger 98 | |
| effects entry | 19 |
| drum: 63, then step 78 or end 18 | |
| normal: 89; kick +27 (+54 when it ends) | |
| arpeggio test 21; with bit 1 +27; timer 0: +51, wrap +36, then +78 | |
| slide test 21; with bit 2 +27; counter 0: +43, negative count +28, then +32; every tick +12 | |
| store 12; trigger test 21; with bit 4 +35, hit +44 | |
| envelope test 21; with bit 3 +27; counter 0: +43; count not 0 +11; negative +36; then +32 | |
| end of effects | 12 |
| return (also after 0x80 and the drum) | 16 |

Command 0x82 (after the dispatch 19 + 65): 55; repeat not 0: +70 (pattern found). Else +32, then for each
sequence byte: read 19; pattern number +70; command bytes +17 and then: 0x80 +22 (stop, then return 16);
otherwise +17; 0xFF +78; otherwise +15; 0x81 +48 plus the song-end test; otherwise +15; 0x82 +26;
otherwise +13; 0x83 +26; other bytes nothing more.

Song-end test: 32, then voice A +47; voice C +76; voice B +112 (+16 when C is inactive); then +21; when all
bytes are 0: +68, then the host's song-end function: `t` is rounded down to an even number and 16 is added;
return +44.

### 6.2 Emulator

| step | cycles |
|---|---|
| head, up to voice A | 110 |
| between voices (A to B, B to C) | 82 |
| per voice: entry 37; tone +117 (+40 noise also on), +25 | |
| per voice: not tone +15; noise path +75; disable +82 | |
| restart needed (tone or noise path): +62, after the restart +10 | |
| after voice C | 37 |
| noise count 0: +21 (then disable if `mode` not 0) | |
| count not 0: +13; 2: +25; 3: +171; then +18 and the noise path | |

Write volume and period: 44 + CV + SV + 12, VOL write; +24, PER write; then 10 + SP. Per channel 0, 1, 2,
3: CV = 163, 180, 187, 214; SV = SP = 79, 110, 141, 149.

Restart (4.3), channel costs ST = 95, 126, 157, 165 (channels 0..3):

| step | cycles |
|---|---|
| entry | 30 |
| line wait: first read of the line | 12 |
| then each read 12; if unchanged +9 and read again; when changed +9 | |
| then each read of `h` 16; below 22: +9 and read again; else +9 | |
| PER = 1 | 16 |
| read DMACON | 12 |
| INTREQ write | 40 |
| DMACON write | 12 |
| interrupt poll: read 12, then 17; repeat until set | |
| LC | 12 |
| LEN | 12 |
| PER | ST + 22 + 20 |
| line wait (as above) | |
| read DMACON | 12 |
| INTREQ write | 52 |
| DMACON write | 12 |
| interrupt poll | |
| line wait | |
| LC, LEN | 12, 12 |
| return | 16 |

The restart is entered right after the "+62" of the tone/noise path or the "+82" of disable.

## 7. Observed gaps

Typical gaps in the logs (cc): PER=1 to INTREQ 26, INTREQ to DMACON 6, LC to LEN 6, LEN to PER about 68
(channel 0) up to 104 (channel 3), VOL to PER 12. After each line wait the next write comes 32..46 cc into
the line. A silent voice costs about 3 raster lines per tick; a channel restarted from a long period can
wait up to one byte of its old period in step 4.

## 8. How to check

`tools/replay-cmp.py REF.log PORT.log` compares tick by tick. The logs in `logs/` (LOGS.md) come from the
reference 68k player run through the host with DELI_WRITE_LOG on synthetic modules. The host calls the
tick routine at the clocks listed in LOGS.md, 70 940 cc apart. With the sample buffers at the base used by
the reference (LOGS.md), every write including its colour clock is reproducible from this text.
