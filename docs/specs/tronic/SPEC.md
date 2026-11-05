# Tronic ("TronicTracker(?)") replayer: behaviour spec

*Sanitised for publication: behaviour only.*

Scope: the module format played by the UADE/Eagleplayer player "Tronic" (name shown by the host:
`TronicTracker(?)`; configured file-name prefixes `dp`, `trc`, `tro`, `tronic`, `tron`). Both modules
found in the corpus carry the suffix `.dp` (`crystal.dp`, `locomotion.dp`); no file with one of the
prefixes is a Tronic module (README).

This is a behaviour description derived from write logs, execution-time histograms and a reading of the
68k reference. It contains no code of the original. Cycle numbers in section 8 are 68000 cycles as the
reference machine charges them (2 cycles = 1 colour clock, "cc").

Status (checked by a private model that follows only this text, against the reference):

| item | result |
|---|---|
| 2 corpus modules, full runs to the song end (10 329 and 11 209 ticks, 246 642 register writes) | value, order and tick-relative time of every write equal the reference |
| 190 synthetic modules (random structurally valid modules that use every effect, both instrument types, all envelope/vibrato/portamento paths, 0x80 and 0xFF list markers, differing list lengths) | 183 equal for the whole run; 7 diverge only from their first undefined read onwards (period index above 85 or a pointer far outside the module, 7.6) |
| paths never executed by a real module | covered by the synthetic modules (effects 3, 4, 7, wave-list jumps, portamento, envelope wrap) |

"Tick-relative time" = colour clock of a write minus the clock at which the tick starts. The tick start
phase and the tick spacing belong to the host (8.1).

## 1. File format

### 1.1 What a module is

A module is a data file, no code and no magic text: four sequence lists followed by seven blocks. All
numbers are big-endian. The replay code lives in the player; the player-resident data it needs (period
table, default instrument) is given in 1.6.

### 1.2 Detection

All of these hold (module-relative offsets; `n` = file size):

- `n >= 20`;
- `L0..L3` = the low 16 bits of the four 32-bit words at offsets 0, 4, 8, 12, each taken as a signed
  16-bit number (lists are shorter than 0x8000 bytes in practice);
- `a = 16 + L0 + L1 + L2 + L3` is even and `a + 2 <= n`;
- `sA` = the 16-bit word at `a` (block A size); `d = a + sA` is even and `d + 8 <= n`;
- the 32-bit word at `d + 4` is `0x005800B0`.

The last test says: the first two entries of the instrument offset table (block B, 1.3) are `0x0058` and
`0x00B0`. The original does the `a + sA` addition on the low 16 bits of the absolute load address, so
a module whose load address makes this sum cross a 64 KiB boundary is judged differently; ignore that.
The check does not validate anything else; a file that passes it with inconsistent blocks has undefined
behaviour (7.6).

### 1.3 Layout

| part | size (bytes) | content |
|---|---|---|
| header | 16 | four 32-bit lengths `L0..L3` (only the low 16 bits count) |
| sequence lists | `L0+L1+L2+L3` | list of voice 0, then voice 1, 2, 3; each a run of 2-byte entries (1.4) |
| block A | `2 + sA` | 16-bit size `sA`, then `sA` bytes: arpeggio lists of 16 bytes each (1.4) |
| block B | `2 + sB` | 16-bit size `sB`, then `sB` bytes: 16-bit instrument offsets (relative to the start of block C); entry `i` (0 based) is `0x58 * (i + 1)` in every module seen |
| block C | `lastB + 0x58` | instrument records of 0x58 bytes (1.5); `lastB` = the last 16-bit word of block B; block C has no size word and its first record (offset 0) is not addressed by any entry of B |
| block D | `2 + sD` | 16-bit size `sD`, then `sD/2` 16-bit byte offsets into block E (wave table index) |
| block E | `2 + sE` | 16-bit size `sE`, then `sE` bytes: wave data, signed 8-bit |
| block F | `4 + sF` | 32-bit size `sF`, then patterns of 64 bytes (16 rows of 4 bytes) |
| block G | 32 | eight 32-bit sample offsets, relative to the sample area |
| sample area | rest of the file | signed 8-bit sample data |

Position of every block follows from the sizes in order (the sequence lists start at offset 16, block A
at `16 + sum(L) `, and so on). Pointers inside the format are offsets from the start of the stated block.

Instrument numbers are 1-based: instrument `k` is the record at `C + B[k-1]`. Instrument 0 reads the
16-bit word just before block B's data (that is `sB` itself) as an offset into C, which is legal
but is not used by sensible modules.

### 1.4 Sequence lists, patterns, arpeggio lists

- Sequence list of voice `v`: `L_v / 2` entries of two bytes: pattern number (byte), transpose (byte).
  The transpose is added to the note modulo 256 (a negative transpose is a byte near 0xFF).
- A pattern is 16 rows of 4 bytes: note (0 = none, 1 to 72 are table entries, 7.6), instrument number
  (1 based, 0 = see 1.3), effect (0 = none, 1 to 8), parameter. Pattern `p` starts at `F + 64 p`.
- Arpeggio list: 16 bytes; each byte is a semitone offset added to the note. A byte `0x80` ends the list
  early. Block A holds `sA / 16` lists; the first byte of a list must not be 0x80 (the player would
  loop forever).

### 1.5 Instrument record (0x58 bytes)

| offset | size | meaning |
|---|---|---|
| 0x00 | u16 | length in words written to AUDxLEN when the instrument starts (both types) |
| 0x02 | u16 | sample type: byte offset of the loop start from the sample start |
| 0x04 | u16 | sample type: loop length in words |
| 0x06 | 5 x 3 bytes | envelope phases: rate, target, hold (5.4); phase `p` at `0x06 + 3p` |
| 0x15 | 5 x 3 bytes | vibrato phases: speed, length, delay (5.3); phase `p` at `0x15 + 3p` |
| 0x24 | u16 | pitch drift: added to the slide term every tick (5.7) |
| 0x26 | byte | type: bit 7 set = sample instrument, clear = wave-list instrument. The byte is also read as an "extra" delay byte by the vibrato phase stepping (5.3) |
| 0x27 | byte | sample instrument: sample number (index into block G); wave-list instrument: wave step interval |
| 0x28 | 48 bytes | wave-list instrument: wave list (5.2): wave numbers 0 to 254 and the jump marker 0xFF followed by a list position |

### 1.6 Player-resident data

Period table `P` (16-bit words, index 0 to 85; the entry used is `P[(note + transpose + arpeggio offset) mod 256]`):

- index 0: 0;
- indices 1 to 12: 6848, 6464, 6096, 5760, 5424, 5120, 4832, 4560, 4304, 4064, 3840, 3616
- indices 13 to 24: 3424, 3232, 3048, 2880, 2712, 2560, 2416, 2280, 2152, 2032, 1920, 1808
- indices 25 to 36: 1712, 1616, 1524, 1440, 1356, 1280, 1208, 1140, 1076, 1016, 960, 904
- indices 37 to 48: 856, 808, 762, 720, 678, 640, 604, 570, 538, 508, 480, 452
- indices 49 to 60: 428, 404, 381, 360, 339, 320, 302, 285, 269, 254, 240, 226
- indices 61 to 72: 214, 202, 190, 180, 170, 160, 151, 143, 135, 127, 120, 113
- indices 73 to 83 and 85: 113; index 84: 0.

(The values are approximately 8 times the PAL 36-note period list in the first 36 entries and the plain list
in the last 36; the exact values above are what the player holds, they are not computed.)

Indices above 85 read other memory of the player (undefined, 7.6). `data/tronic_periods.bin` holds the
86 words.

Default instrument: the state of a voice before its first note points at a built-in instrument record
of 46 bytes (`data/tronic_default_instrument.bin`). Used fields: length 4 words; envelope phase 0 =
(rate 0, target 1, hold 255) so the envelope never moves from level 0 (silence); vibrato phase 0 =
(speed 0, length 1, delay 1) and phases 1 to 4 = (speed 2, length 2, delay 2); drift 0; type byte 0
(wave-list instrument); wave step interval 254; wave list bytes: 0, 0xFF, 0 (and a zero after it).

### 1.7 Subsongs, title, length

One song per module, no title or author. The only size figure is the number of sequence entries of the
longest list, `max(L_v) / 2`. The song-end event (7.5) is the only loop point; the first voice that
wraps its list reports it. With a constant speed `s` the entry `k` of a list is fetched at tick
`(s + 1) + 16 (s + 1) k` (first tick = 1) and the wrap is reported when the last entry is fetched; speed
effects change this, so the exact time comes from running the replay.

## 2. Replayer state

Global: `speed` (initial 5), `counter` (initial 0, signed byte), `master` (initial 0x3F), and a scratch
word `S` (7.3).

Per voice `v` (0 to 3), initial values in brackets:

| field | width | meaning |
|---|---|---|
| seqpos | u16 | byte position in the voice's sequence list [0] |
| seqend | u16 | `L_v` |
| row | u16 | byte offset of the current row inside the pattern, 0 to 60 step 4 [0] |
| pat | pointer | current pattern base [unused until the first fetch] |
| inst | pointer | current instrument record [the default instrument, 1.6] |
| pend | byte | "loop to be set next tick" flag (copy of instrument byte 0x26 taken at a note start) [0] |
| wwait | byte | wave-list wait counter [0] |
| wpos | byte | wave-list position [0] |
| cur | u16 | current period [0] |
| tgt | u16 | target period of the last note [0] |
| note | byte | last note [0] |
| trans | byte | transpose of the last fetched sequence entry [0] |
| maxvol | byte | volume cap [0x3F] |
| slide | u16 | slide term set by effects 3 and 4 [0] |
| level | u16 | envelope level [0] |
| epos | u16 | envelope phase offset, 0, 3, 6, ... [0] |
| ehold | byte | envelope hold counter [0] |
| pspeed | byte | portamento speed, 0 = off [0] |
| vdir | byte | vibrato direction flag [0] |
| vacc | u16 | accumulated pitch offset (vibrato, slide, drift) [0] |
| vcnt | byte | vibrato length counter [0] |
| vphase | byte | vibrato phase offset 0, 3, 6, 9, 12 [0] |
| vdelay | byte | vibrato phase delay counter [0] |
| abase | pointer | arpeggio list in use [start of block A] |
| aidx | u16 | position inside the arpeggio list, 0 to 15 [0] |

## 3. Tick structure

A tick (called every 70 940 cc, 8.1) does, in this order:

1. `counter := counter - 1` (byte). If the result is negative (bit 7 set), `counter := speed`.
2. For voice 0, 1, 2, 3: the voice routine (4, 5, 6).
3. DMACON write of `0x8000 | (OR of the DMA bits (1, 2, 4, 8) of the four voices)`; every voice
   contributes its bit on every tick, so the value is always `0x800F`.

A row is processed in a tick when `counter == 0` after step 1. With `speed = s` there is one row every
`s + 1` ticks; the first row of a song is processed in tick 6 (counting the first tick as 1, at the
default speed 5).

## 4. Voice routine

The voice routine for channel `c` does, in this order (every tick unless stated):

1. If `pend` has bit 7 set: the loop of a sample instrument is set (4.1).
2. If `counter == 0`: row processing (4.2).
3. Wave list (5.2), vibrato (5.3), envelope (5.4), portamento (5.5), arpeggio and period (5.6, 5.7),
   volume (5.8).
4. The voice's DMA bit is added to the DMACON value of 3.3.

### 4.1 Loop set (sample instruments)

When `pend` bit 7 is set: `pend := 0`; write AUDxLEN with the instrument's loop length (word at 0x04);
set the first word of the sample (at the sample start) to zero in memory; write AUDxLC with
`sample start + loop start offset (word at 0x02)`. The sample start is `S0 + G[sample number]` where `S0` is
the start of the sample area, `G[k]` the 32-bit entry `k` of block G, and the sample number is byte 0x27
of the instrument.

### 4.2 Row processing

1. If `row == 0`: fetch the next sequence entry. The pair at `seqpos` gives the pattern number and
   `trans`; `pat := F + 64 * pattern`; `S` low word := `seqpos + 2` (7.3); `seqpos := seqpos + 2`;
   if `(seqpos - seqend) mod 65536` has bit 15 clear, `seqpos := 0` and the song-end notification is
   given for this voice (7.5).
2. Read the row (`pat + row`): note, instrument number, effect, parameter.
3. If `note != 0`: new note (4.3).
4. If `effect` is in 1 to 8: effect (4.4).
5. `row := (row + 4) mod 64`.

### 4.3 New note

In this order:

1. Write DMACON with the voice's DMA bit alone (`1`, `2`, `4` or `8`; bit 15 clear: DMA of the channel
   off).
2. `note := note`; `tgt := P[(note + trans) mod 256]`.
3. `S := (instrument * 2 - 2) mod 65536` (upper half cleared). The instrument record is
   `C + word at (B + signed16(S))`; `inst :=` that record. `pend := byte 0x26` of the record.
4. If `pend` has bit 7 set (sample instrument): write AUDxLC with the sample start (4.1), then write
   AUDxLEN with the record's length (word at 0x00).
5. Reset: `wwait := 0`, `wpos := 0`, `level := 0`, `epos := 0`, `ehold := 0`, `aidx := 0`,
   `vdir := 0`, `vacc := 0`, `vcnt := byte 0x16`, `vphase := 0`, `vdelay := byte 0x17`. Not reset:
   `cur`, `slide`, `pspeed`, `maxvol`, `abase`, `master`.

(Note that the sample instrument's first LEN/LC writes happen now, the loop is set at the start of the
next tick by 4.1, and DMA is turned back on at the end of this tick by the DMACON write of 3.3.)

### 4.4 Effects

`S` low byte := parameter (7.3), then by effect number:

| effect | action |
|---|---|
| 1 | `speed := parameter` |
| 2 | nothing |
| 3 | `S` low word := `-(S low word) mod 65536`; `slide := S low word` (note: the high byte of the operand is the leftover scratch byte, 7.3) |
| 4 | `slide := S low word` (same remark) |
| 5 | `pspeed := parameter` |
| 6 | `maxvol := parameter` |
| 7 | `master := parameter` |
| 8 | `S := S * 16` (32 bit); `abase := A + S` (block A start plus the shifted scratch value, i.e. list number `parameter` when the scratch high bytes are zero) |

Effect numbers 9 to 0x80 jump into player code that is not an effect (undefined, do not use);
bytes above 0x80 are treated as no effect.

## 5. Per-tick processing

### 5.1 Order

Wave list, vibrato, envelope, portamento, arpeggio/period, volume: as listed. All counters are bytes or
words with wrap-around as stated.

### 5.2 Wave list (wave-list instruments only: bit 7 of byte 0x26 clear)

- If `wwait != 0`: `wwait := wwait - 1` (byte). Done.
- Else `wwait := byte 0x27`. Then take the list at record offset 0x28, position `i = wpos` (byte):
  - `S` low byte := `i`. If `list[i] == 0xFF`: `i := list[i + 1]`, and test `list[i]` again (the scratch
    byte is set to `i` on every pass). If it is 0xFF again the wave list is abandoned for this tick:
    nothing is written and `wpos` is unchanged.
  - With wave number `w = list[i]`: offset `o = signed16(word at D + 2 w)`; write AUDxLC with `E + o`; write
    AUDxLEN with the record's length (word at 0x00); `wpos := (i + 1) mod 256`; `S` low byte := `wpos`.

Because `wpos` is a byte and the list has 48 bytes, a list that is not terminated by a 0xFF marker runs
on into the following record bytes.

### 5.3 Vibrato

Phase record: record bytes `0x15 + vphase` (speed), `+1` (length), `+2` (delay).

1. If `vdir == 0` then `vacc := vacc + speed` else `vacc := vacc - speed` (16 bit).
2. `vcnt := vcnt - 1` (byte). If the result is 0: `vcnt := length`; `vdir := 0xFF - vdir` (flip between 0
   and 0xFF).
3. If `vdelay != 0`: `vdelay := vdelay - 1`. Else: `vphase := vphase + 3` (byte); if `vphase == 15`,
   `vphase := 12`; then `vdelay :=` the record byte at `0x15 + (old vphase) + 5` (that is the delay byte of
   the following phase; for the last phase it is the type byte 0x26).

### 5.4 Envelope

Phase record: bytes `0x06 + epos` (rate), `+1` (target), `+2` (hold).

- If `ehold != 0`: `ehold := ehold - 1`. Nothing else.
- Else `S := target` (upper bits cleared). Let `diff = (target - level) mod 65536`.
  - If `diff` has bit 15 clear (target >= level, equality included): `level := level + rate` (16 bit).
    If `(target - level) mod 65536` now has bit 15 set (level passed the target): `level := target`;
    `epos := epos + 3`; if `epos == 15` then `epos := 12`; `ehold := hold`.
  - Else (target below level): `level := level - rate`. If `(target - level) mod 65536` has bit 15
    clear (level reached or passed the target): `level := target`; `epos := epos + 3` (no wrap test
    here, so it can reach 15 and beyond and read record bytes of the vibrato phases); `ehold := hold`.

A phase with rate 0 and target equal to the level never ends. With rate 0 and a target above the level
it never ends either.

### 5.5 Portamento (only if `pspeed != 0`)

`S` low word := `tgt`. If `(tgt - cur) mod 65536` has bit 15 clear: `cur := cur + pspeed`; if
`(tgt - cur) mod 65536` now has bit 15 set: `cur := tgt`. Else: `cur := cur - pspeed`; if
`(tgt - cur) mod 65536` has bit 15 clear: `cur := tgt`.

### 5.6 Arpeggio list and base period

Read the list at `abase`: `e = list[aidx]`, with `S` low word := `aidx` before the read; if `e == 0x80`:
`aidx := 0` and read again (a list whose first byte is 0x80 never ends). Then `aidx := (aidx + 1) mod 16`.
Base period: if `pspeed != 0`, `base := cur`; otherwise `base := P[(e + note + trans) mod 256]` and
`cur := base`.

### 5.7 Period write

`d := (word 0x24 of the record - slide) mod 65536`; `S` low word := `d`; `vacc := vacc - d`;
`period := (base + vacc) mod 65536`; write AUDxPER with it.

So with drift 0 the slide term adds `slide` to `vacc` every tick; vibrato adds to the same sum.

### 5.8 Volume

`v = (level >> 2) & 0x3F`. Apply the caps: if bit 7 of `(v - maxvol) mod 256` is clear then `v := maxvol`;
then if bit 7 of `(v - master) mod 256` is clear then `v := master`. Write AUDxVOL with `v & 0x7F`
(the reference's volume stage multiplies by the host gain 64 and shifts right by 6: identity at the
defaults used for all logs).

## 6. Write order

### 6.1 Per tick

For each voice in order 0, 1, 2, 3, the writes of the voice that occur:

1. Loop set (4.1): AUDxLEN (loop length), then AUDxLC (32 bit, both halves at the same stamp).
2. New note (4.3): DMACON (channel bit), then for sample instruments AUDxLC, AUDxLEN.
3. Wave list (5.2): AUDxLC, then AUDxLEN.
4. AUDxPER, then AUDxVOL.

After the four voices: DMACON `0x800F`. A tick therefore always contains 4 PER, 4 VOL and one
DMACON write. In the log AUDxLC appears as two lines (high half, low half) with the same stamp.

### 6.2 Initialisation (sound start)

The host calls the player's volume routine twice before the sound start; each call writes AUDxVOL
(0 at that time) for channels 0 to 3 at gaps of 156, 168, 162 cc. Then the sound start does, with
gaps measured from its first write (cc):

| offset | write |
|---|---|
| 0 | DMACON `0x000F` |
| 12, 20, 28, 36 | AUDxLC of channel 0, 1, 2, 3: a silent 4-word area inside the player (not module data) |
| 44, 52, 60, 68 | AUDxLEN of channel 0, 1, 2, 3: 4 |
| 78, 86, 94, 102 | AUDxPER and AUDxVOL of channel 0, 1, 2, 3, both 0, same stamp |

All voice fields get their initial values (2). The first tick is called later by the host (8.1).

### 6.3 Sound stop

(Read from the player, not exercised by the logs.) DMACON `0x000F`, then AUDxPER and AUDxVOL set to 0 of
all four channels.

## 7. Details and quirks

### 7.1 The default instrument before the first note

Until a voice has played a note it uses the default instrument (1.6). Its wave list steps once at the
first tick (writes LC = `E + D[0]`, LEN = 4) and then every 255 ticks; its envelope stays at 0 (volume
0) and its vibrato makes PER climb by 2 per tick at the beginning (visible in the logs as PER 2, 4, 6 ...).

### 7.2 Sample memory

4.1 zeroes the first word of the sample every time the loop is set, so the first word of a sample is
silent after its first note. Waveform data in block E is not modified.

### 7.3 The scratch word S

The effect operands 3, 4 and 8 use a 16/32-bit value in which only the low byte is the parameter; the
rest is whatever earlier steps left. `S` is a 32-bit value updated at these points and nowhere else:

1. at the start of every tick `S := 0` (the host's value in all reference runs);
2. sequence fetch (4.2): low word := `seqpos + 2`;
3. new note (4.3): `S := (instrument * 2 - 2) mod 65536`;
4. effect (4.4): low byte := parameter; effect 3 negates the low word; effect 8 shifts the whole 32-bit
   value left by 4;
5. wave list (5.2): low byte := list position `i`, later `(i + 1) mod 256`;
6. envelope (5.4), only when `ehold == 0`: `S := target`;
7. portamento (5.5): low word := `tgt`;
8. arpeggio read (5.6): low word := `aidx` (before the read);
9. period (5.7): low word := `d`.

Consequence: an effect 3 or 4 operand has a non-zero high byte when the voice had no new note in the
row and the previous voice (or the previous step of the same voice) left a negative or large value; effect
8 then points far outside block A (undefined, 7.6).

### 7.4 Voice order and channel mapping

Voice `v` drives Paula channel `v`. There is no stereo logic in the player.

### 7.5 Song end

Each time a voice's sequence list wraps (4.2 step 1) the song-end notification of the host is given
once, during that voice's processing (its cycle cost, 8.5, shifts the voices after it). The player keeps
playing. Nothing else changes in the register writes. The reference host stops its render 1 to 4 ticks
after the first notification.

### 7.6 Undefined behaviour (do not rely on it)

- period table index above 85 (note + transpose + arpeggio offset): the reference reads other player
  memory;
- effect 8 with a scratch high part, instrument number above the number of records, a pattern or sample
  number outside the module, a wave number whose D/E pointers leave the module: the reference reads memory
  outside the file. In the synthetic runs a read past the end of the file returned 0 (so a model that returns
  0 is exact there), but a pointer that lands in other player or host memory returns that memory;
- effect numbers 9 to 0x80, a first arpeggio list byte of 0x80, `L_v = 0`.

## 8. Timing

### 8.1 Frame

Ticks are called every 70 940 cc exactly (50.00 Hz at 3 546 895 Hz), verified over 10 000+ ticks of both
modules. The first tick of the reference runs starts at cc 73 949 after reset (host property). There
is no wrapper cost in front of the first player step.

### 8.2 Clock model

All costs are static 68000 cycles (the reference charges the average of taken and not-taken for
conditional branches, and a constant for the volume multiplication). For a tick keep a counter `T` of
cycles, starting at 0 at the tick start. A write is stamped `tick_start + floor(T / 2)` where `T`
already includes the step that makes the write (the cost of a write is part of the step cost below). Both
halves of a 32-bit AUDxLC write get the same stamp.

Song-end notification (7.5): its step `SE_CALL` (16 cycles) re-bases the clock: from then
on `stamp = floor(T_at / 2) + floor((T - T_at) / 2)` where `T_at` is `T` just after the notification.
Several notifications in one tick apply this repeatedly.

### 8.3 Helper blocks (channel-dependent costs)

The reference stores AUDxLC, AUDxLEN and AUDxPER copies through small routines whose cost depends on
the channel `c`, and writes AUDxVOL through a volume stage. These costs include the call itself:

| id | cycles | meaning |
|---|---|---|
| `HLC0` | 105 | store of an LC value, channel 0 |
| `HLC1` | 136 | channel 1 |
| `HLC2` | 167 | channel 2 |
| `HLC3` | 175 | channel 3 |
| `HLEN0` | 101 | store of a LEN or PER value, channel 0 |
| `HLEN1` | 132 | channel 1 |
| `HLEN2` | 163 | channel 2 |
| `HLEN3` | 171 | channel 3 |
| `VOLW0` | 195 | volume stage from its call up to and including the AUDxVOL write, channel 0 |
| `VOLW1` | 218 | channel 1 |
| `VOLW2` | 241 | channel 2 |
| `VOLW3` | 231 | channel 3 |
| `VOLR` | 44 | volume stage, from the AUDxVOL write to its return |

### 8.4 Tick skeleton

| id | cycles | step |
|---|---|---|
| `T_ENTRY` | 273 | from the tick start to the end of the counter decrement test |
| `T_RELOAD` | 20 | added when the counter went negative and is reloaded from `speed` |
| `T_VCALL` | 26 | before each of the four voice routines |
| `T_DMA` | 20 | the DMACON write (stamp after this step) |
| `T_CLOSE` | 168 | closing steps after the DMACON write (no writes) |

### 8.5 Voice steps

A step is added to `T` when its condition holds. "(W ...)" marks a step whose end is the stamp of the
write named. Where a helper block follows a step, add its 8.3 value for the voice's channel.

| id | cycles | condition / content |
|---|---|---|
| `V_HEAD` | 49 | always, first |
| `P_A` | 118 | `pend` bit 7 set: up to the AUDxLEN write (W LEN); then `HLEN` |
| `P_C` | 48 | same case: up to the AUDxLC write (W LC); then `HLC` |
| `V_CNT` | 23 | always: counter test |
| `R_A` | 21 | `counter == 0` |
| `R_SEQ` | 159 | `row == 0`: sequence fetch |
| `R_WRAP` | 92 | the fetch wrapped (before the notification) |
| `SE_CALL` | 16 | the song-end notification (then the clock is re-based, 8.2) |
| `R_B` | 63 | `counter == 0`: row read and note test |
| `N_A` | 24 | new note: DMACON write (W DMACON) |
| `N_B` | 185 | new note: period target, instrument lookup, type test |
| `N_C` | 90 | new sample-instrument note: up to the AUDxLC write (W LC); then `HLC` |
| `N_E` | 20 | same: up to the AUDxLEN write (W LEN); then `HLEN` |
| `N_G` | 152 | new note: field resets |
| `R_C` | 29 | `counter == 0`: effect fetch and test |
| `R_D` | 30 | effect 1 to 8 present: dispatch |
| `E1` | 32 | effect 1 |
| `E2` | 33 | effect 2 |
| `E3` | 36 | effect 3 |
| `E4` | 32 | effect 4 |
| `E5` | 32 | effect 5 |
| `E6` | 32 | effect 6 |
| `E7` | 32 | effect 7 |
| `E8` | 66 | effect 8 |
| `R_E` | 36 | `counter == 0`: row advance |
| `W_A` | 21 | always |
| `W_B` | 21 | wave-list instrument |
| `W_C` | 26 | wave-list instrument, `wwait != 0` |
| `W_D` | 48 | wave-list instrument, `wwait == 0` |
| `W_E` | 35 | each pass of the list read |
| `W_F` | 45 | marker 0xFF seen: jump |
| `W_ABORT` | 10 | second marker: wave step abandoned (no writes) |
| `W_G` | 86 | wave write: up to the AUDxLC write (W LC); then `HLC` |
| `W_I` | 32 | up to the AUDxLEN write (W LEN); then `HLEN` |
| `W_K` | 16 | wave write finished |
| `X_A` | 61 | vibrato: start |
| `X_B` | 26 | `vdir == 0` |
| `X_C` | 16 | `vdir != 0` |
| `X_D` | 25 | length count |
| `X_E` | 36 | length reached 0 |
| `X_F` | 21 | delay test |
| `X_G` | 26 | `vdelay != 0` |
| `X_H` | 41 | `vdelay == 0`: phase step |
| `X_I` | 16 | phase reached 15 |
| `X_J` | 20 | `vdelay == 0`: delay reload |
| `Y_A` | 21 | envelope: hold test |
| `Y_B` | 26 | `ehold != 0` |
| `Y_C` | 77 | `ehold == 0`: phase read and comparison |
| `Y_D` | 37 | rising branch |
| `Y_E` | 53 | rising branch, target passed |
| `Y_F` | 16 | rising branch, `epos` reached 15 |
| `Y_G` | 20 | rising branch, hold reload |
| `Y_H` | 37 | falling branch |
| `Y_I` | 58 | falling branch, target reached |
| `Z_A` | 25 | portamento test (always) |
| `Z_B` | 33 | `pspeed != 0`: start |
| `Z_C` | 37 | `target >= cur` |
| `Z_D` | 12 | `target >= cur`, clamped |
| `Z_E` | 37 | `target < cur` |
| `Z_F` | 22 | `target < cur`, clamped |
| `A_A` | 63 | arpeggio list read (every read) |
| `A_B` | 26 | the entry was 0x80 (then `A_A` again) |
| `A_C` | 36 | index advance |
| `A_D` | 21 | portamento test |
| `A_E` | 22 | `pspeed != 0` |
| `A_F` | 62 | `pspeed == 0`: table lookup |
| `PE_A` | 64 | period computation up to the AUDxPER write (W PER); then `HLEN` |
| `VO_A` | 51 | volume: level, first cap test |
| `VO_B` | 12 | first cap applied (`maxvol`) |
| `VO_C` | 21 | second cap test |
| `VO_D` | 12 | second cap applied (`master`) |
| `VO_F` | 40 | voice end (after `VOLW` and `VOLR`) |

### 8.6 Order of the steps in one voice routine

1. `V_HEAD`. If `pend`: `P_A` (W LEN), `HLEN`, `P_C` (W LC), `HLC`.
2. `V_CNT`. If `counter == 0`: `R_A`; if `row == 0`: `R_SEQ` (and, if wrapped, `R_WRAP`, `SE_CALL`); `R_B`;
   if a note: `N_A` (W DMACON), `N_B`, for a sample instrument `N_C` (W LC), `HLC`, `N_E` (W LEN), `HLEN`,
   then `N_G`; `R_C`; if an effect 1 to 8: `R_D` and the effect's own step (`E1` to `E8`); `R_E`.
3. `W_A`; for a wave-list instrument: `W_B`, then `W_C` (wait running) or the wave step: `W_D`, then passes
   `W_E` (for every marker seen `W_F`, and after a marker a further `W_E` if the entry it jumped to is a normal
   entry, or `W_ABORT` and the end of the wave step if it is a second marker); a completed wave step then
   `W_G` (W LC), `HLC`, `W_I` (W LEN), `HLEN`, `W_K`.
4. Vibrato: `X_A`; `X_B` or `X_C`; `X_D`; `X_E` if the length reached 0; `X_F`; then `X_G`, or `X_H` (plus `X_I` if
   the phase reached 15) and `X_J`.
5. Envelope: `Y_A`; then `Y_B`, or `Y_C` followed by `Y_D` (plus `Y_E`, `Y_F` if `epos` hit 15, and `Y_G` when the
   target was passed) or `Y_H` (plus `Y_I` when the target was reached).
6. Portamento: `Z_A`; if `pspeed != 0`: `Z_B`, then `Z_C` (plus `Z_D` if clamped) or `Z_E` (plus `Z_F`).
7. Arpeggio: `A_A` (and `A_B` followed by another `A_A` per 0x80 hit), `A_C`, `A_D`, then `A_E` or `A_F`.
8. `PE_A` (W PER), `HLEN`.
9. `VO_A`, `VO_B` if capped, `VO_C`, `VO_D` if capped, `VOLW` (W VOL), `VOLR`, `VO_F`.

A tick: `T_ENTRY`, `T_RELOAD` if the counter reloaded, then for each voice `T_VCALL` and the voice's steps, then
`T_DMA` (W DMACON `0x800F`) and `T_CLOSE`.
