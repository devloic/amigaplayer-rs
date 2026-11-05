# AProSys: behaviour specification

*Sanitised for publication: behaviour only.*

AProSys (Petter A. Urkedal, 1989-1990; DeliTracker player "AProSys", file prefix `aps.`) is a
four-voice real-time synthesizer. A module holds only data: a block sequence, four-voice blocks,
track rows, arpeggio tables, instruments with four envelopes, chains of "event records" that play
sample data or drive a per-tick waveform generator, and the sample data itself. The replayer
generates synthetic waveforms into 128-byte chip-memory buffers every tick, and plays sample chains
with Paula audio interrupts.

This document describes behaviour only. Numbers are decimal unless written `0x..`. "Byte",
"word" and "long" are 8, 16 and 32 bits, big-endian in the module.

## 1. Arithmetic conventions

The replayer works in fixed-size integers and its results depend on wrap-around. Unless stated
otherwise:

- `s8(x)`, `s16(x)`: the low 8 / 16 bits of `x` read as two's-complement signed.
- `u8`, `u16`: the low 8 / 16 bits unsigned. "mod 256" / "mod 65536" mean the same.
- A product of two 16-bit operands is exact (32-bit). "`>> n`" on a signed value is an
  arithmetic shift (floor division by 2^n).
- **Division** `q16(n, d)`: `n` a signed 32-bit dividend, `d` a signed 16-bit divisor (never 0
  where used). The quotient is truncated toward zero. If it does not fit in −32768..32767 the
  result is the low 16 bits of the dividend instead (the dividend is left unchanged). Only the
  16-bit result is used.
- Byte stores keep the low 8 bits.

## 2. Module file

### 2.1 Detection and chunks

- Bytes 0-3 are `ADRV` and bytes 4-7 are `PACK`. Nothing else is checked by detection.
- From offset 8 there are **nine chunks**, each a long length `L` followed by `L` bytes. At
  start-up the player walks the nine chunks and refuses the module if any chunk ends past the
  end of the file. Pointers into the module are absolute addresses of the chunk data. Because the
  player reads words and longs, chunks are expected to start at even offsets (the corpus tools
  in `modules/` pad every chunk to an even length).

| chunk | content | record |
|---|---|---|
| 0 | sequence | 2 bytes per entry |
| 1 | blocks | 26 bytes per block |
| 2 | track rows | 4 bytes per row |
| 3 | arpeggio bytes | 1 byte |
| 4 | instruments | 26 bytes |
| 5 | envelope points | 2 bytes per point |
| 6 | event records | 6 bytes (long `E`, word `W`) |
| 7 | sample data | signed bytes, addressed in words |
| 8 | not used by the replayer | |

There is no title, no subsong table and no stored length. There is one song: playback always
starts at sequence entry 0. Its length can only be found by playing it (§9).

### 2.2 Sequence entry (chunk 0)

Two bytes `c, p`. The command is `c & 0xE0`, the count `n = c & 0x1F`:

| command | meaning |
|---|---|
| `0x00` | end of song |
| `0x20` | play `n` consecutive blocks starting at block `p` (blocks `p`, `p+1`, ..., `p+n−1`); `n = 0` plays nothing |
| `0x40` | loop: jump by `p` (signed, see §5.3) until this entry has been met `n` times |
| `0x60` | jump: new position = (position + `p`) mod 256 |
| `0x80` | transpose: the period table starts `p & 63` entries further (§4) |
| `0xA0` | speed multiplier = `p` |
| `0xC0` | audio filter: `p ≠ 0` switches it on, `p = 0` off |
| `0xE0` | nothing |

### 2.3 Block (chunk 1, 26 bytes)

| offset | meaning |
|---|---|
| 0 | tick multiplier `B` (ticks per row = `B ×` speed multiplier, mod 256) |
| 1 | number of rows |
| 2 + 6v | voice `v` (0-3) entry, 6 bytes: |
| +0 | bits 0-5: transpose (added to the row's note); value 63 mutes the voice for the block (every row is treated as "no note"); bit 7: the voice's fourth envelope restarts on a triggered note in row 0 (§6.4) |
| +1 | instrument base number |
| +2 | bits 0-5: block volume `bv` (channel volume target = `4 × bv`); bits 6-7: volume slide step |
| +3 | arpeggio offset added to the row's arpeggio start |
| +4 | word: first track row of the voice (row index into chunk 2) |

### 2.4 Track row (chunk 2, 4 bytes)

| byte | bits | meaning |
|---|---|---|
| 0 | 0-5 | note 0-62; 63 = no note |
| 0 | 6 | arpeggio mode for this row (byte 3 is the arpeggio start) |
| 0 | 7 | trigger (restart envelopes and, for interrupt-played events, restart the sound) |
| 1 | | instrument / event offset (see §5.5) |
| 2 | 0-5 | note volume factor, or a row-length override when bit 6 is set |
| 2 | 6 | note volume = channel volume and row length = `(bits 0-5) × speed multiplier` |
| 2 | 7 | event-offset mode (§5.5) |
| 3 | | arpeggio start (bit 6 of byte 0 set) or portamento speed (bit 6 clear) |

### 2.5 Arpeggio bytes (chunk 3)

Read one per tick (§6.2): a byte with bit 7 clear is a note offset (bits 0-5 used, bit 6
ignored); a byte with bit 7 set is a jump: bits 0-6 as a signed 7-bit number (−64..63) of bytes,
relative to the jump byte itself.

### 2.6 Instrument (chunk 4, 26 bytes)

| offset | size | meaning |
|---|---|---|
| 0 | byte | event record number for a note (attack) |
| 1 | byte | event record number for the release |
| 2 | byte | not used |
| 3 | byte | envelope 0 time scale (0 = envelope 0 does not run) |
| 4 | byte | envelope 2 time scale (0 = off) |
| 5 | byte | envelope 3 time scale (0 = off) |
| 6 | word | envelope 0 start point (point index in chunk 5) |
| 8 | word | envelope 0 release start point |
| 10 | word | envelope 1 start point |
| 12 | word | envelope 2 start point |
| 14 | word | envelope 3 start point |
| 16 | byte | envelope 1 base time scale (0 = envelope 1 does not run) |
| 17 | byte | envelope 1 time-scale modulation by envelope 2 |
| 18 | s8 | detune (subtracted from the period) |
| 19 | s8 | pitch depth 1 |
| 20 | s8 | pitch depth 2 |
| 21 | s8 | volume base |
| 22 | s8 | volume depth 1 |
| 23 | s8 | volume depth 2 |
| 24 | byte | pitch routing: bits 4-7 envelope `A`, bits 2-3 envelope `B`, bits 0-1 envelope `C` |
| 25 | byte | volume routing: bits 2-7 envelope `A2`, bits 0-1 envelope `B2` |

Envelope numbers in the routing bytes select one of the voice's four envelopes 0-3. Larger
numbers (possible in the high fields) continue into the following voices' envelopes: number
`k ≥ 4` of voice `v` is envelope `k mod 4` of voice `v + k div 4`. Past voice 3 the value is
host dependent (§11).

### 2.7 Envelope point (chunk 5, 2 bytes)

`dur` (unsigned byte), `level` (signed byte). A point with `dur = 0` is a jump: the next point
read is `level` points (signed) from this one. Envelope values are 16-bit with the level in the
high byte (`level × 256`).

### 2.8 Event record (chunk 6, 6 bytes)

A long `E` and a word `W`. The type is `E >> 28`:

| type | name | used as |
|---|---|---|
| 0 | loop | play `W` words from sample offset `E & 0xFFFFFF` (words into chunk 7), repeat forever |
| 1 | one-shot | play once, then silence |
| 2 | chain | play, then continue with the next record |
| 3 | synth | waveform generator (§8) |
| 4 | moving sample A | sample whose start moves every tick (§7) |
| 5 | moving sample B | same, other formula |
| 13 | stop | silence |
| other | | nothing |

Sample offset `o = E & 0xFFFFFF` addresses chunk 7 in words: the byte address is
`chunk7 + 2 × o` (32-bit arithmetic; offsets may make it point outside the module).

For types 4, 5 and 3, bits 26-27 of `E` select an envelope `eA` and bits 24-25 an envelope `eB`
of the voice. Records following a synth record are read as parameter records (§8).

## 3. Player state

Global: sequence position (byte), block pointer, block repetition counter, row index (byte), row
timer (byte), ticks per row (byte), speed multiplier (byte, starts 0), loop counter (byte, starts
0xFF = idle), transpose (starts 0), jump budget (byte), a "playing" flag.

Per voice: channel volume (byte, starts 0), track row position, note (byte), current instrument,
note volume (byte), arpeggio position, portamento speed (byte), event position, current period
(word, starts 0), "note active" flag, "key" flag, "fresh note" flag, two filter states (words,
start 0), four envelopes (position, counter, value, delta: words, start 0), and two 128-byte
synthesis buffers ("front" and "back").

The synthesis buffers are one 1024-byte chip-memory area holding eight buffers: voice `v` starts
with front = buffer `v`, back = buffer `v + 4`. They are allocated without clearing (in the
reference runs the area was zero). A separate 2-byte zeroed chip buffer is the "silence" sample.
Two 128-byte work buffers (in the player's own memory, starting zero) are used by one synthesis
operation.

**Period table** (48 words, index = transpose + note [+ arpeggio offset]):

```
1710 1614 1524 1438 1357 1281 1209 1141 1077 1017  960  906
 855  807  762  719  679  641  605  571  539  508  480  453
 428  404  381  360  339  320  302  285  269  254  240  226
 214  202  190  180  170  160  151  143  135  127  120  113
```

Indices past 47 read the player's following data: 48 and 49 are the two halves of an address
(host dependent), 50-53 are 128, 256, 512, 1024, and beyond 53 host dependent.

## 4. Start

Playing starts at sequence position 0 with the jump budget at 0 (it wraps to 255 at the first
jump, so 255 jumps succeed during start), all envelope counters 0. There is no voice update and no row increment: the sequencer runs
(§5.3), the repetition counter is set to 0 and §5.2 continues at step 3's test with the block
pointer still on block `p` (the first block starts without moving the pointer), so the first row
is processed (§5.4). The interrupt runs that the first row requests (§10) happen right after.

## 5. Tick

The host calls the tick 50 times a second. Each tick:

1. The jump budget is set to 255.
2. **Voice update** for voices 0 to 3 (§6).
3. The row timer is decremented (mod 256). If it is not 0 the tick ends.
4. Otherwise the song **advances** (§5.2). If the advance reaches the end of the song (§9), the
   player writes INTENA `0x0780` and DMACON `0x000F` (only the first time) and the tick ends.

Audio interrupts requested during the tick are served after it, lowest channel first (§10).

### 5.1 Jump budget

Every jump (sequence jump, loop jump, arpeggio jump, envelope jump) first decrements the budget
(mod 256); when the result is 0 the jump is abandoned (so in a tick the 255th jump fails):

- in the sequencer: the song ends (§9);
- in an arpeggio: the rest of the voice update is skipped for this and all following voices;
- in an envelope: the rest of this voice's update is skipped (no period, volume or sample write
  for the voice this tick); the following voices are updated. The budget keeps wrapping (the
  next jump makes it 255 again).

### 5.2 Advance

1. Every voice's track row position moves one row on; the row index increases by 1.
2. If the row index is below the block's row count: process the row (§5.4), set the row timer to
   the ticks per row; if that is 0, advance again at once (step 1). Otherwise done.
3. Else the block pointer moves to the next block record (26 bytes on) and the repetition
   counter increases. If the counter is below the current sequence entry's count `n`: **start
   the block** (ticks per row = `B ×` speed multiplier mod 256; each voice's track row position =
   its first track row), set the row index to 0 and continue at step 2 (a block of 0 rows goes
   straight to the next one).
4. Else the sequence position increases by 1 and the sequencer runs (§5.3). At a play entry the
   repetition counter is set to 0 and step 3's test runs (with the block pointer at block `p`;
   a count of 0 moves on to the next sequence entry).

The ticks-per-row value set at block start stays until the next block start, except that a note
row with byte 2 bit 6 set replaces it (§5.4); the replacement also stays.

### 5.3 Sequencer

From the current position, entries are executed until a play entry or the end:

- `0x00`: end of song.
- `0x20`: the block pointer = block `p`; the sequencer stops here (the position stays on this
  entry; its count is read by §5.2).
- `0x40`: if the loop counter is 0xFF it is loaded with `n`. It is decremented; if it reaches 0
  it becomes 0xFF again and the position moves to the next entry. Otherwise the entry acts as a
  `0x60` jump. (One loop counter for the whole song; `n = 0` loops forever.)
- `0x60`: position = (position + `p`) mod 256, using the jump budget, and the entry there is
  executed next.
- `0x80`: transpose = `p & 63`; `0xA0`: speed multiplier = `p`; `0xC0`: filter; `0xE0`:
  nothing. Each then moves to the next entry.

The filter command sets (off) or clears (on) bit 1 of CIA-A port A, leaving the other bits (in
the reference logs the byte written is 0x02 for off and 0x00 for on).

### 5.4 Row processing

First DMACON `0x8200` is written. Then for each voice `v` (with its block entry `b` and its
current track row `r`):

1. The key flag is cleared.
2. **Channel volume**: target `t = 4 × (b[2] & 63)`, step `s = b[2] >> 6`. If `s = 0` the
   channel volume becomes `t`. Otherwise if it is below `t` it rises by `s` (mod 256) and becomes
   `t` if it reaches or passes it; if it is above `t` it falls by `s` (mod 256) and becomes `t`
   if the result is not above `t`. (Comparisons unsigned; a fall below 0 wraps to a large value
   that is then kept.)
3. If `r[0]` bit 6 is set the arpeggio position = chunk3 + `r[3]` + `b[3]`; otherwise the
   portamento speed = `r[3]`. This happens on every row, with or without a note.
4. **No note** if `r[0] & 63 = 63` or `b[0] & 63 = 63`: if the voice's note is active it becomes
   inactive and the release happens (§5.6). Nothing else for this voice.
5. **Note**: the note becomes active; note = `(r[0] & 63) + (b[0] & 63)` (mod 256).
   - Instrument and event position: if `r[2]` bit 7 is set: instrument = `b[1]`, event record =
     instrument byte 0 + `r[1]`. Otherwise instrument = `b[1] + r[1]`, event record =
     instrument byte 0.
   - If `r[0]` bit 7 is set: **retrigger** (§5.7).
   - Note volume: if `r[2]` bit 6 is set, note volume = channel volume and ticks per row =
     `(r[2] & 63) ×` speed multiplier (mod 256); otherwise note volume =
     `((r[2] & 63) × channel volume) >> 5` (mod 256).
   - By the type of the event record at the event position:
     - **4 or 5**: AUDxLEN = `W & 0xFF`; AUDxLOC = chunk7 + 2 × (`E & 0xFFFFFF`); INTENA
       disable for the channel (`0x0080 << v`); DMACON `0x8000 | 1 << v`.
     - **3**: AUDxLEN = `E & 0xFF`; AUDxLOC = the voice's front buffer; INTENA disable for the
       channel; DMACON `0x8000 | 1 << v`.
     - any other type, if `r[0]` bit 7 is set: the key flag is set and DMACON `1 << v` (DMA off)
       is written. Without bit 7 nothing is written; the new event position is picked up by the
       channel's next audio interrupt.

After all four voices, for each voice whose key flag is set, in voice order: INTENA
`0x8000 | 0x0080 << v` (enable), then AUDxDAT = 0. This requests the channel's audio interrupt
(§10).

### 5.5 Instrument / event selection summary

The instrument record is chunk4 + 26 × number; the event record is chunk6 + 6 × number. Byte
`r[1]` thus either picks a neighbouring instrument (bit 7 of `r[2]` clear) or a neighbouring
event record of the block's instrument (bit 7 set).

### 5.6 Release (no-note row on an active note)

If the current instrument's bytes 0 and 1 differ, the instrument and event position change: with
`r[2]` bit 7 set, instrument = `b[1]` and event record = (new instrument byte 1) + `r[1]`;
otherwise instrument = `b[1] + r[1]` and event record = new instrument byte 1. No sound is
restarted; the next audio interrupt of the channel (if any) uses the new event position.

Then, in all cases, envelope 0 enters its release: its position = the (current) instrument's
release start point `R`; counter = `(dur(R) × scale0) mod 65536` where scale0 = instrument
byte 3; if the counter is not 0, delta = `q16(s16(level(R) × 256 − value), s16(counter))`. The
value is kept (so the envelope ramps from where it is).

### 5.7 Retrigger

For envelopes 0, 1 and 2 (start points at instrument words 6, 10, 12): position = start point,
value = `level(start) × 256` (mod 65536), counter = 0. Deltas are kept (they are overwritten
before use). Envelope 3 (start word 14) restarts the same way only when the row index is 0 and
the voice's block entry has bit 7 of byte 0 set; otherwise it keeps running. The fresh-note flag
is set.

## 6. Voice update (every tick, voices 0-3)

1. If the channel volume is 0: AUDxVOL = 0 is written and nothing else happens for the voice.
2. **Period.** If the current track row has bit 6 of byte 0 set (arpeggio): read arpeggio bytes
   from the arpeggio position, following jumps (§2.5, each jump uses the budget); the first
   offset byte `x` gives period = table[transpose + (x & 63) + note], and the arpeggio position
   moves past it. Otherwise (portamento): target = table[transpose + note]; with speed 0 the
   period becomes the target; otherwise the current period moves toward the target by the speed
   (16-bit, unsigned comparisons, wrap on the fall) and becomes the target when it reaches or
   passes it.
3. **Envelopes** step in the order 0, 1, 2, 3 (§6.1), each only if its scale is non-zero:
   envelope 0 scale = instrument byte 3; envelope 1 runs if byte 16 is non-zero; envelope 2
   scale = byte 4; envelope 3 scale = byte 5.
4. **Pitch.** With `V[k]` = `s16` of envelope `k`'s value and the routing of byte 24:
   - `a = (V[A] × depth1) >> 8`
   - `b = (s16(a) × V[B]) >> 16`
   - `c = (V[C] × depth2) >> 10`
   - AUDxPER = `(u16(period − detune) − (b + c)) mod 65536`.
5. **Volume.** With the routing of byte 25:
   `a = (V[A2] × vdepth1 + V[B2] × vdepth2) >> 8`, then `a = a + 16 × vbase`,
   `b = (s16(a) × note volume) >> 12`; AUDxVOL = `(b & 127) × hostvol / 64`, where hostvol is
   the host's per-channel volume (64 for all four channels in the reference, so AUDxVOL =
   `b & 127`).
6. **Event action** by the type of the record at the event position:
   - 4: AUDxLOC = chunk7 + 2 × (`(E & 0xFFFFFF) + f4`) (§7);
   - 5: AUDxLOC = chunk7 + 2 × (`(E & 0xFFFFFF) + f5`);
   - 3: synthesize into the front buffer (§8), clear the fresh-note flag, swap front and back,
     and write AUDxLOC = the buffer just generated (now the back buffer);
   - other: nothing.

So the writes of a voice are, in order: PER, VOL, and then LOC for types 3, 4, 5.

### 6.1 Envelope step

1. value = value + delta (mod 65536).
2. If the counter was not 0 it is decremented and the step ends.
3. If the counter was 0: value = `level(P) × 256` of the point at the position (mod 65536), and
   the position moves to the next point. Then, repeatedly: counter = `(dur × S) mod 65536` for
   the point now at the position, with the scale `S`; if the counter is not 0, delta =
   `q16(s16(level × 256 − value), s16(counter))` and the step ends; if it is 0 the point is a
   jump: use the jump budget (§5.1) and move the position by `level` points.

The envelope position always points at the point the current ramp is heading for (the release
sets it to `R` in the same sense). So each point is reached exactly (its level is set when its time is over) and the value ramps
linearly toward the next point's level over `dur × S` ticks.

**Envelope 1's scale** is not constant: `S = ((byte17 × u16(value of envelope 2)) mod 65536 >> 8)
+ byte16 + k`, where `k` is 1 for the first point loaded in this step and increases by 1 for each
jump followed in the same step (mod 65536). Envelope 2's value is the one before envelope 2 steps
this tick.

### 6.2 Arpeggio jumps and abandoned updates

See §5.1 for what an exhausted jump budget skips. An arpeggio table must reach an offset byte
within the budget; an envelope that loops through zero-duration points only escapes on the next
tick (the step restarts from the jump point: its level is taken as the value and the position
moves past it).

## 7. Moving sample start (types 4 and 5)

For a record `(E, W)` on voice `v`, with `V[eA]`, `V[eB]` the selected envelopes' values:

- `f4`: `a = (V[eA] × (W >> 8)) >> 8`; `a = (s16(a) × V[eB]) >> 16`; `f4 = (W & 0xFF) × s16(a)`.
- `f5`: `a = (V[eA] × (W >> 8)) >> 8`; `f5 = (W & 0xFF) × s16(a)`.

The result is a signed number of words added to the sample offset. (At row time the start is
written without this term, §5.4.) Note that `W & 0xFF` is also the sample length at row time.

## 8. Synthesis (type 3)

A synth record `(E, W)` at event position `X` on voice `v`:

- buffer length `n` = `2 × (E & 63)` bytes, or 128 when `E & 63 = 0` (the AUDxLEN written at row
  time is `E & 0xFF` words, so modules keep bits 6-7 clear);
- mode = bits 20-21, operation = bits 16-19, comb record = bits 8-11, filter record = bits 12-15;
- `buf` = the voice's front buffer; `buf[i]` signed bytes.

Generation (mode), then the comb stage, then the filter stage, all in place in `buf`.

### 8.1 Mode 0: generators

`A = s16((V[eA] × (W >> 8)) >> 8)`, `Bv = s16((V[eB] × (W & 0xFF)) >> 8)`.

- op 0, pulse: `d = s16(A + Bv) >> 4`. If `d` (as unsigned 16-bit) is less than `n`, the
  buffer gets `d` bytes of 100 then `n − d` bytes of −100; otherwise it is left as it is.
- op 1, staircase: `y = Bv >> 4`, `x = A >> 4`, `z = (127 − x − y) mod 256`, `k = n >> 3`.
  Eight runs of `k` bytes each, values (mod 256): `x+z+y`, `x+z−y`, `x−z+y`, `x−z−y`, `y−x+z`,
  `z−x−y`, `y−x−z`, `−x−z−y`. The last `n mod 8` bytes are left as they are.
- op 2, ramp: `d = s16(A + Bv) >> 4`; `buf[i] = d × (n − 1 − i)` (low byte) for `i = 0..n−1`.
- other ops: nothing.

### 8.2 Mode 1: one source

The source record is `X + 1` (`E2`, `W2`). Source address by its type: 1: only if the voice's
fresh-note flag is set (otherwise no generation at all this tick; comb and filter still run),
address chunk7 + 2 × (`E2 & 0xFFFFFF`); 4: chunk7 + 2 × (`(E2 & 0xFFFFFF) + f4(E2, W2)`);
5: same with `f5`; any other type: chunk7 + 2 × (`E2 & 0xFFFFFF`). `S[i]` = signed source bytes.

- op 0: `buf[i] = S[i]`.
- op 1: `buf[i] = (buf[i] >> 1) + S[i]`.
- op 2 (`n ≥ 2`): `buf[0] = (buf[n−1] >> 1) + S[0]`, then for `i = 1..n−1`:
  `buf[i] = (buf[i−1] >> 1) + S[i]` (using the bytes just written).
- op 3 (`n ≥ 3`): `buf[0] = (buf[n−2] >> 1) + S[0]`, `buf[1] = (buf[n−1] >> 1) + S[1]`, then for
  `i = 2..n−1`: `buf[i] = (buf[i−2] >> 1) + S[i]`.
- op 4: three passes of the "halve" transform `H`: work1 = H(S), work2 = H(work1),
  buf = H(work2), where for input `I` and output `O` of `n` bytes, `h = n / 2`:
  `O[i] = I[i] >> 1` for all `i`; then `O[i] += I[2i] >> 1` for `i < h`; then
  `O[h + i] += I[2i] >> 2` for `i < h` (byte arithmetic).
- other ops: nothing.

`>> 1` and `>> 2` here are arithmetic shifts of the signed byte; sums are stored mod 256.

### 8.3 Mode 2: two sources

Source 1 from record `X + 1`, source 2 from record `X + 2`, addresses as in mode 1 but without the
fresh-note condition for type 1. `S1[i]`, `S2[i]` signed bytes; results stored mod 256.

- op 1: `(S1[i] + S2[i]) >> 1`
- op 2: `(S1[i] × S2[i]) >> 6`
- op 3, 4, 5: bitwise and, or, exclusive or of the two bytes
- op 6: the byte at (source 2 address + `S1[i]`) (signed offset)
- op 7: a running sum `acc` (starting 0) adds `S1[i]`; the byte at (source 2 address + `acc`)
- op 9: `(S1[i] + S2[i] + buf[i]) >> 1`
- ops 0, 8 and 10-15 read through an address the original never sets (host dependent, §11).

### 8.4 Comb stage

If the comb record number `k = (E >> 8) & 15` is non-zero, the parameter record is `X + k`
(`P`, `PW`):

- `g = s16(((V[P bits 26-27] × (PW >> 12)) >> 8) + ((P >> 8) & 0xFF))`
- `h = s16(((V[P bits 24-25] × (PW & 0xFF)) >> 12) + (P & 0xFF))`

Three indices `i`, `j`, `m` all start at `n`. A "pass" repeats, while `m ≠ 0`:
`buf[i] = s16(buf[i] × g + buf[j] × 256 + buf[m] × h) >> 8` (all sums 16-bit; the
products are taken as their low 16 bits; the 16-bit result is shifted arithmetically and its low
byte stored); then `j −= 2`, `i −= 1`, `m −= 4`.

Four passes run: before pass 1 nothing is reset, before pass 2 `m = n`, before pass 3 `j = n` and
`m = n`, before pass 4 `m = n`. (`i` runs from `n` down to 1 over the four passes; `buf[n]` is the
first byte of the next buffer in the 1024-byte area, so the comb stage writes one byte into the
neighbouring buffer; for buffer 7 that byte is outside the area, host dependent.) `n` must be a
multiple of 4; otherwise the original never leaves the stage.

### 8.5 Filter stage

If the filter record number `f = (E >> 12) & 15` is non-zero, the parameter record is `X + f`
(`Q`, `QW`). Bit 22 of `Q`: if set and the fresh-note flag is set, both filter states become 0.

- `g1 = s16(((V[Q bits 26-27] × s8(QW >> 8)) >> 10) + ((Q >> 8) & 0xFF))`
- `g2 = s16(((V[Q bits 24-25] × s8(QW & 0xFF)) >> 10) + (Q & 0xFF))`

With `F1`, `F2` the voice's filter states (16-bit):

**Bit 23 of Q clear** (two one-pole stages):

1. If `g1 ≠ 0`: for `i = 0..n−1`: `q = q16(s16(F1 × 8), g1)`; `buf[i] = buf[i] − q` (mod 256);
   `F1 = F1 + buf[i]` (signed byte, mod 65536). (F1 is stored only when this runs.)
2. If `g2 ≠ 0`: for `i = 0..n−1`: `q = q16(s16(F2 × 8), g2)`; `buf[i] = q` (low byte);
   `F2 = F2 + (buf[i+1] − q)` (with `buf[i+1]` the byte not yet rewritten; for `i = n−1` it is
   `buf[n]`, see §8.4), 16-bit. (F2 stored only when this runs.)

**Bit 23 of Q set** (state-variable stage): `d = s16(−g1)`; if `d = 0` or `g2 = 0` nothing happens.
Otherwise for `i = 0..n−1`:
`q1 = q16(s16(F2 × 8), g2)`; `x = buf[i] − q1` (16-bit); `q2 = q16(s16(q1) × 8, d)` (a 32-bit
dividend; overflow gives its low 16 bits); `F1 = F1 + q2`; `x = x − F1`; `F2 = F2 + x` (all
16-bit); `buf[i] = q2` (low byte). Both states are stored.

### 8.6 After synthesis

The fresh-note flag is cleared, front and back are exchanged, and AUDxLOC is written with the
buffer just generated. A synth voice therefore alternates between its two buffers every tick;
operations that read `buf` (feedback, comb, filter) see that buffer's contents from two ticks
before, and the row-time AUDxLOC (§5.4) points at the buffer that the next tick will generate.

## 9. Song end

The song ends when the sequencer meets an end entry, or when a sequence jump exhausts the jump
budget. In a tick this writes INTENA `0x0780` and DMACON `0x000F` (all audio interrupts and DMA
off) once. The player does not tell the host; the reference keeps calling the tick, and the
player keeps running voice updates (AUDxPER/AUDxVOL/AUDxLOC every tick as before, the row timer
at 0 runs down from 255). After 256 more ticks it advances again, past the end entry, reading
whatever follows; that part is undefined (host and module layout dependent) and a port should
consider the song ended at the end tick. An end met during start writes nothing.

## 10. Audio interrupts

Each channel has a handler. The engine raises it when Paula requests the channel's interrupt
while it is enabled (the AUDxDAT write after a key, §5.4, or later, when the channel's DMA reloads
its sample pointer). Requests made during a tick are served after the tick, lowest channel first.
The handler for channel `v`:

1. INTENA `0x0080 << v` (disable).
2. By the type of the record `(E, W)` at the voice's event position (read before any change of
   the event position):
   - **1**: event position = the internal stop record; AUDxLOC = chunk7 + 2 × (`E & 0xFFFFFF`);
     AUDxLEN = `W`; DMACON `0x8000 | 1 << v`; INTENA `0x8000 | 0x0080 << v`.
   - **2**: event position moves to the next record; AUDxLOC, AUDxLEN = `W` as for type 1;
     DMACON on; INTENA enable.
   - **0**: AUDxLOC, AUDxLEN = `W`; INTENA `0x0080 << v` (again); DMACON on.
   - **13** (and the internal stop record): AUDxLOC = the 2-byte silence buffer; AUDxLEN = 1;
     INTENA `0x0080 << v`.
   - other types: nothing more.

AUDxLEN is the whole word `W` here (at row time only its low byte). AUDxLOC is written as two
words (high, low).

In the reference host, the channel is still in Paula's manual mode after the AUDxDAT write when
the handler turns DMA on, so it fetches from its previous internal pointer and length until those
run out, and only then loads the new location and requests the next interrupt. If DMA of a channel
has not yet stopped when AUDxDAT is written (a very long period keeps the last word playing), no
interrupt is raised at that moment. All of this is Paula behaviour: a port that makes the same
writes and serves the interrupts the engine reports reproduces it.

## 11. Host-dependent cases

These depend on host memory or allocation and are not defined by the module:

- period table indices 48, 49 and above 53 (§3);
- envelope numbers that run past voice 3 (§2.6);
- mode 2 ops 0, 8, 10-15 (an uninitialised address; its value is left over from earlier
  calls of the synthesis routine);
- the byte past buffer 7 in the comb and filter stages; the initial contents of the
  synthesis area (zero in the reference);
- reading instrument, event or envelope data through a pointer that was never set (a voice with
  a non-zero channel volume before its first note, an envelope 3 that never restarted, an
  arpeggio row before any arpeggio start);
- sample or source addresses outside the module;
- the per-channel host volume (64 in the reference);
- everything after the song end (§9).

## 12. Write order summary

- Start: `[CIA filter bit]`, DMACON `0x8200`, row writes (§5.4), interrupt runs (§10).
- Tick: per voice 0-3: VOL 0 (channel volume 0) or PER, VOL, [LOC]; then, on a row: DMACON
  `0x8200`, per voice row writes, the INTENA/AUDxDAT pairs of keyed voices; on song end INTENA
  `0x0780`, DMACON `0x000F`; then interrupt runs.

Timing is in LOGS.md.
