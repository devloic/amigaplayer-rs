# EarAche (Amiga, `EA.*`) replayer: behaviour spec

*Sanitised for publication: behaviour only.*

EarAche is a 4-voice music format by Morten Grouleff (1990). Modules hold song data, instrument
records, volume and pitch tables and raw 8-bit samples; the replay engine is not part of the module.
This spec describes the behaviour of the reference player (the Wanted Team Eagleplayer "EarAche",
version 1.05 player, as shipped in the UADE players directory) as observed in its Paula register
writes and their timing. It contains no code of the original. Times are in colour clocks ("cc") or
in CPU cycles ("u", 1 u = 0.5 cc).

Status (checked by a private model that follows only this text, against the reference):

| item | result |
|---|---|
| 41 synthetic modules used to measure the timing constants, 36 798 register writes | value, order and absolute colour clock of every write equal the reference |
| 12 further synthetic modules built after the constants were fixed (held out; random header kind, shuffled sample table), 12 745 writes | equal |
| real modules | none available (see README) |

## 1. File format

All multi-byte values are big-endian. "Word" = 16 bits, "long" = 32 bits.

### 1.1 Data start

The song data ("data block") starts at file offset `D`:

- first word is 0: `D = 0`;
- the file starts with the 4 bytes `EASO`: `D = 4`;
- otherwise (a "branch header", see 1.2): word at offset 2 is `d1`; the 10-byte group at `T = 2 + d1`
  is followed by a word `d3` at `T + 10`; `D = T + 10 + d3`.

All offsets below are relative to `D` unless said otherwise. The module's position in memory is
called `module_base` (the address where the file is loaded); `data_base = module_base + D`.

### 1.2 Detection

Let `p` = the candidate data start:

- If the first word is 0: `p = 0`.
- Else if the first long is `EASO`: `p = 4`.
- Else the file must have a branch header:
  - word 0 = the branch-opcode word (value in `data/branch_header_key.bin`, first word); word at 2 = `d1`, with `d1` non-zero, positive as a signed word, even;
  - word at 4 = the same branch-opcode word; word at 6 non-zero, positive as a signed word, even (not used further);
  - at `T = 2 + d1`: the five words of the key sequence in `data/branch_header_key.bin` (second to sixth words of the file) in this order; then the word at
    `T + 10` = `d3`, non-zero, positive as a signed word, even. `p = T + 10 + d3`.
  - Anything else: reject.

Then, with the longs `h0..h5` at `p + 0, 4, ..., 20`:

- `h0 == 0x18`;
- `h1 - h0` (signed 32-bit) is greater than 0 and a multiple of 8;
- `h4 - h2` (signed 32-bit) is greater than 0 and a multiple of 16.

Otherwise reject. The UADE file name prefixes are `EA.` and `MG.`; detection does not use the name.

### 1.3 Header

Six longs at `D + 0 .. D + 20`, each an offset from `D`:

| long | name | content |
|---|---|---|
| h0 | `rows` | row area (always 0x18) |
| h1 | `order` | order list |
| h2 | `instr` | instrument records, 16 bytes each |
| h3 | `smptab` | sample table, 4 bytes per entry |
| h4 | `envtab` | volume tables, 64 bytes each |
| h5 | `pittab` | pitch tables, 64 bytes each |

### 1.4 Rows

Row `r` is 8 bytes at `rows + r * 8`: one word per channel, channel 0 first. A word of 0 means
nothing happens on that channel. Otherwise its high byte is the note `n` (0..255) and its low byte the
instrument number `i` (0..255). The row offset is computed as `(r * 8) & 0xFFFF` and added as a signed
16-bit value (row numbers are at most 0xFFF, so this is `r * 8` for real use).

### 1.5 Order list

A sequence of entries of two words each, at `order + 4 * k` for entry `k`:

- normal entry: word A = first row (low 12 bits), word B = last row (low 12 bits) and speed
  (bits 12..15). The rows `first .. last` are played in order (both inclusive), each row lasting
  `speed` ticks (speed 0 means 256 ticks).
- end marker: word A = 0xFFFF; word B = jump target (entry index, low 12 bits).

Section 3.2 gives the exact way the list is read, including the unusual cases.

### 1.6 Instrument record (16 bytes, `I[0..15]` at `instr + i * 16`)

| byte | meaning |
|---|---|
| I[0] | sample number (bit 7 ignored) |
| I[1] | note length: 0 = sample plays once (see 3.3), otherwise the voice is cut after `I[1]` ticks |
| I[2] | volume table number (bit 7 ignored) |
| I[3] | bit 7 clear: pitch table number (all 8 bits; bit 7 is clear); bit 7 set: arpeggio mode, bits 0..6 = third arpeggio offset |
| I[4] | volume table: initial delay (ticks) |
| I[5] | volume table: speed (ticks per step) |
| I[6] | pitch table: initial delay; arpeggio mode: first arpeggio offset |
| I[7] | pitch table: speed; arpeggio mode: second arpeggio offset |
| I[8] | unused |
| I[9] | start volume (bit 7 ignored, so 0..127) |
| I[10] | fine tune, signed byte |
| I[11..15] | unused |

### 1.7 Sample table

Entry `s` at `smptab + 4 * s`: word `start` (in units of 4 bytes, from `D`), word `length` (in words).
The sample is at `data_base + 4 * start`. The table ends at the first entry whose long is 0 (only the
module info and the start-up cost (section 4.1) use the count; playback indexes the table directly).

### 1.8 Volume and pitch tables

Volume table `k`: 64 bytes at `envtab + 64 * k`. Pitch table `k`: 64 bytes at `pittab + 64 * k`.
Bytes are read one per step (3.4, 3.5); a pointer is not bounded by the 64 bytes.

### 1.9 Period table

The player's own period table, indexed by note (71 entries, index 0..70):

```
 0: 0x000
 1: 0xD60 0xCA0 0xBEA 0xB40 0xAA0 0x9FC 0x976 0x8EE 0x86E 0x7F4 0x782 0x716
13: 0x6B0 0x650 0x5F4 0x5A0 0x54E 0x502 0x4BA 0x476 0x436 0x3F9 0x3C0 0x38A
25: 0x358 0x328 0x2FA 0x2D0 0x2A6 0x280 0x25C 0x23A 0x21A 0x1FC 0x1E0 0x1C5
37: 0x1AC 0x194 0x17D 0x168 0x153 0x140 0x12E 0x11D 0x10D 0x0FE 0x0F0 0x0E2
49: 0x0D6 0x0CA 0x0BE 0x0B4 0x0AA 0x0A0 0x097 0x08F 0x087 0x07F 0x078
60..70: 0x071 (all eleven)
```

Index 71 reads the player's tick counter and speed as one word: `(counter << 8) | speed`, with the
values current at that moment (section 2; during a row tick the counter has already been reloaded,
so it equals `speed` there). Indices above 71 read the player's per-voice state and code (which
contain absolute memory addresses of the reference); they are not specified (README).

### 1.10 Info

One subsong, no title. Song length (for display) = the number of entries from the start of the
order list up to the first entry whose first word is 0xFFFF. Number of samples = number of sample
table entries before the zero long.

## 2. State

Global: `row` (word), `last` (word), `pos` (address of the next order entry), `counter` (byte),
`speed` (byte).

Per voice `c` (0..3), each starting at 0 / off:

- `vcount`, `vspeed` (bytes); `vptr`, `vloop` (addresses into a volume table)
- `pcount`, `pspeed` (bytes; also the first two arpeggio offsets), `arp3` (byte), `astep` (byte)
- `pptr`, `ploop` (addresses into a pitch table); `arp` (flag)
- `note` (word), `period` (word; 0 = voice inactive), `cut` (byte)

Start of playing: `row = 0`, `last = 0`, `pos = order` (first entry), `counter = 1`, `speed = 1`.
Voice state is all zero on a fresh load (the reference does not clear it again on a restart in the
same process).

Paula channel `c` registers: AUDxLC, AUDxLEN, AUDxPER, AUDxVOL; DMA bit of channel `c` is `1 << c`.

## 3. Per tick

The player is called once per tick (section 4 for the times). Each tick:

1. `counter = (counter - 1) & 0xFF`. If it is now 0: `counter = speed` and run the **row step** (3.2).
2. Run the **voice update** (3.4, 3.5) for the voices in the order **2, 0, 1, 3**.

### 3.2 Row step

1. `row = row + 1` (16 bits). If `row <= last` (signed 16-bit compare), go to step 3 with this row.
2. Otherwise read the order list at `pos` (the reading pointer `a` advances by 2 per word read):
   1. Read word `x`. If `x != 0xFFFF`, go to 2.4.
   2. End marker: report **song end**. Then repeat at most 16 times: read word `j` (the jump target),
      set `a = order + 4 * (j & 0xFFF)`, read word `x`; if `x != 0xFFFF` go to 2.4.
      (So a chain of end markers is followed through their jump words.)
   3. After 16 jumps that all landed on end markers: skip one word (`a = a + 2`) and go back to 2.1.
   4. `row = x & 0xFFF`. Read word `y`: `last = y & 0xFFF`, `speed = counter = (y >> 12) & 15`.
      `pos = a` (the entry after the one used).
3. For channels `c = 0, 1, 2, 3` in order: take word `w` of row `row`; if non-zero, start a note
   (3.3) with note `n = w >> 8`, instrument `I` = record `w & 0xFF`.

Consequences: on the first tick the counter goes 1 -> 0, so the first tick is a row tick, `row`
becomes 1 > `last` = 0, and the first order entry is read. A new entry's first row is played on the
tick that reads it. Song end is reported once for every end marker read in 2.1 (a tick can report it
several times); the song keeps playing. The reference host stops its render a few ticks after the
first report (LOGS.md).

### 3.3 Note start (channel `c`, Paula registers of `c`, DMA bit `b = 1 << c`)

Writes and state changes, in this order:

1. AUDxPER = 0x0078.
2. DMACON = `b` (channel off).
3. Sample entry `S` = entry `I[0] & 0x7F` of the sample table. AUDxLEN = `S.length`.
4. AUDxLC = `data_base + 4 * S.start` (32-bit; logged as two words at the same clock).
5. `vptr = vloop = envtab + 64 * (I[2] & 0x7F)`.
6. If `I[3]` has bit 7 set: `arp = 1`, `arp3 = I[3] & 0x7F`, `astep = 2`. Otherwise: `arp = 0`,
   `pptr = ploop = pittab + 64 * I[3]`. (The mode is only the `arp` flag; the arpeggio fields and the
   pitch-table fields are separate state here.)
7. `vcount = I[4]`, `vspeed = I[5]`, `pcount = I[6]`, `pspeed = I[7]`, `note = n`.
8. Base period: `q = n / 12` (integer), `s = (5 - q) & 7`; `f = I[10]` as signed; if `s != 0`,
   `f = (f << s)` as a 16-bit value. `period = (table[n] + f) & 0xFFFF`.
   (Notes 0..11 shift the fine tune by 5, 12..23 by 4, ..., 48..59 by 1, 60..71 not at all.)
9. AUDxVOL = `I[9] & 0x7F` (values above 64 are written as they are).
10. Wait (4.4).
11. AUDxPER = `period`.
12. DMACON = `0x8000 | b` (channel on).
13. If `I[1] == 0`: AUDxLEN = 1 and `cut = 0` (the sample plays once, then Paula repeats its
    first word). Otherwise `cut = (I[1] + 1) & 0xFF` (so `I[1] = 255` gives `cut = 0`: never cut, and
    no AUDxLEN write).

Note 0 gives `period = f << 5`; with `f = 0` the period is 0, the voice counts as inactive (3.4) and
AUDxPER = 0 is written.

### 3.4 Voice update, part 1: cut and volume table (voice `c`)

If `period == 0`, the voice is skipped entirely this tick.

1. Cut: if `cut != 0`: `cut = cut - 1`; if it became 0: DMACON = `b` (off), AUDxLC = address of 4
   zero bytes in chip memory (in the reference a fixed buffer of the player, 0x007A03C0 in the logs),
   and `period = 0`. The rest of this update still runs for this tick. The voice is inactive from the
   next tick on, unless a pitch-table step (3.5) happens later in this same update: it computes
   `p = 0 + x`, clamps to 100 and stores it in `period`, which makes the voice active again (it keeps
   being updated, with DMA off). An arpeggio write does not change `period`.
2. Volume table: if `vcount != 0`: `vcount = vcount - 1`; if it became 0, read bytes at `vptr`
   (advancing a local pointer):
   - byte `<= 0x40`: AUDxVOL = byte; `vptr` = pointer after it; `vcount = vspeed`.
   - byte `== 0xFF`: continue reading at `vloop` (the start of the table). A table whose first
     byte is 0xFF never ends (the original hangs; not specified).
   - any other byte (0x41..0xFE): `vcount = 0` (the table stops; `vptr` is not changed).
   So the first table value is written on tick `I[4]` counted from the note-start tick as tick 1
   (`I[4] = 1`: on the note-start tick itself, right after the note start); `I[4] = 0` never
   starts the table; `vspeed = 0` stops it after the first value.

### 3.5 Voice update, part 2: pitch (voice `c`, still only if `period` was non-zero at 3.4)

Arpeggio mode (`arp = 1`):

- `astep = (astep + 1) & 3`. If `astep != 0`, `o` = (`pcount`, `pspeed`, `arp3`) for `astep` =
  (1, 2, 3); if that `o` is 0: `astep = 2`. If `astep` was 0 after the increment, `o = 0`.
- AUDxPER = `table[(o + note) & 0x7FF]` — every tick. The base `period` (with fine tune) is not
  used here, only as the active flag. Fine tune therefore has no effect in arpeggio mode except at
  the note start.
- The sequence of offsets is thus 0, o1, o2, o3, 0, o1, ... with a zero `o1` or `o2` jumping to step
  3 on the next tick, and a zero `o3` making every following tick use offset 0.
- After a note start `astep = 2`, so the note-start tick's own update uses `o3`.

Pitch-table mode (`arp = 0`):

- If `pcount == 0`: nothing.
- Else `pcount = pcount - 1`; if it became 0: read byte `x` at `pptr` (signed). If `x > 0x40`
  (0x41..0x7F), read instead the byte at `ploop` (the table start), and the pointer continues after
  that byte (no check on that byte). `p = period + x` (signed 16-bit); if `p < 100` (signed),
  `p = 100`. AUDxPER = `p`; `period = p` (the change accumulates); `pptr` = pointer after the byte
  used; `pcount = pspeed`.
- Bytes 0x80..0xFF are negative steps, 0x00..0x40 positive steps. `pspeed = 0` stops after one step.

### 3.6 Write order summary

Row tick: for each channel with a note (0..3): PER(0x78), DMACON(off), LEN, LC, VOL, [wait], PER,
DMACON(on), [LEN(1)]. Then voices 2, 0, 1, 3: [DMACON(off), LC(silence)] (cut), [VOL] (volume
table), [PER] (pitch). Non-row tick: only the voice part.

### 3.7 Song end

Reported in 3.2 step 2.2. Playing continues with the jump target; all voice state carries over.

## 4. Timing

### 4.1 Tick times in the reference host

Ticks come from a timer every **70 940 cc**. Tick `k` starts at `E(k) = E0 + 70 940 * k`, the moment
the host enters the player. In the reference host (clock 0 = emulated reset):

`E0 = 71 897 + C + J`, with

- `C` (detection) = 101 (plain data start), 124 (`EASO`), 255 (branch header);
- `J` (initialisation) = `floor((465 + 47 * L + 84 * N - 20 * K + H) / 2)`, where `L` = song length
  (1.10), `N` = number of samples (1.10), `K` = number of sample table entries (among the `N`) whose
  `start` word is smaller than the largest `start` word of the entries before it (unsigned
  compare; a table in ascending order gives `K = 0`), `H` = 0 (plain), 29 (`EASO`), 63 (branch
  header).

So E0 depends on the module, not on the file path (checked with module paths of 16 and 233 characters:
identical logs). LOGS.md lists E0 for every log.

All times below are in u (CPU cycles) counted from `E` of the tick. A write at `u` lands on colour
clock `E + floor(u / 2)`. The beam line at clock `t` is `floor(t / 227)`; only changes of the line
number matter (4.4).

### 4.2 Row tick, channel part

Start: `u = 418` if the row continues inside the current order entry (3.2 step 1), or
`u = 595 + 145 * R + 97 * J + 6 * F + 53 * X` if the order list is read, where `R` = end markers read
in step 2.1 (song-end reports), `J` = all jumps followed (2.2), `F` = those of the `J` jumps that landed on another end
marker, `X` = skips (2.3).

Then for channel `c = 0..3`:

- no note: `u += NN[c]`.
- note:
  - `u += 133`; PER(0x78) at `u`
  - `u += 24`; DMACON(off) at `u`
  - `u += 118`; LEN at `u`
  - `u += LC[c]`; LC at `u`
  - `u += VO[c] - 22 * A - 14 * Z`, where `A` = 1 in arpeggio mode, `Z` = 1 for notes with no
    fine-tune shift (`s = 0` in 3.3 step 8, i.e. notes 60..71); VOL at `u`
  - `u += WS[c]`; the wait starts at `u` and ends at `r` (4.4)
  - `u = r + 75`; PER at `u`
  - `u += 36`; DMACON(on) at `u`
  - if `I[1] == 0`: `u += 39`; LEN(1) at `u`; `u += NL[c]`. Otherwise `u += NS[c]`.

| c | NN | LC | VO | WS | NL | NS |
|---|---|---|---|---|---|---|
| 0 | 81 | 137 | 864 | 215 | 18 | 31 |
| 1 | 81 | 168 | 912 | 246 | 18 | 31 |
| 2 | 63 | 199 | 950 | 277 | 0 | 13 |
| 3 | 137 | 207 | 985 | 285 | 74 | 87 |

After channel 3, `u` is the start of the voice part (4.3). On a non-row tick the voice part starts
at `u = 305`.

### 4.3 Voice part

For the voices in update order (2, 0, 1, 3), slot `i = 0..3`: if `i > 0`, `u += 50`. Then for
voice `c`:

- inactive (`period == 0`): `u += 27`; next voice.
- cut (3.4 step 1): `cut == 0`: `u += 105`. Counted down, not yet 0: `u += 124`. Reaching 0:
  `u += 103`; DMACON(off) at `u`; `u += 24`; LC(silence) at `u`; `u += 61`.
- volume table (3.4 step 2), add in this order:
  - `vcount == 0`: `u += 0`
  - counted down, not yet 0: `u += 27`
  - step: `u += 80` for every 0xFF byte met (jump to the table start); then
    - value: `u += EV[c]`; VOL at `u`; `u += EP[c]`
    - stop byte: `u += 111`
- pitch (3.5):
  - arpeggio: `u += 142` (step 0), `175` (step 1..3 with non-zero offset), `191` (zero offset met);
    PER at `u`.
  - pitch table: `pcount == 0`: `u += 26`. Counted down, not yet 0: `u += 57`. Step: `u += 149`,
    plus 24 if the byte was > 0x40 (read from the table start), plus 8 if clamped to 100; PER at
    `u`; `u += 42`.

| c | EV | EP |
|---|---|---|
| 0 | 229 | 175 |
| 1 | 246 | 206 |
| 2 | 253 | 237 |
| 3 | 280 | 245 |

### 4.4 The line wait (inside every note start)

The player waits for 9 changes of the beam's line number by polling it. Started at `u`:

```
r1 = u
repeat 9 times:
    L = line at r1
    r = r1 + 16
    while line at r == L: r = r + 25
    (r is the poll that saw the change)
    r1 = r + 35            (not after the 9th change)
the wait ends at r (the poll that saw the 9th change)
```

"line at x" is `floor((E + floor(x / 2)) / 227)`. The wait takes between about 8 and 9 lines
(1 816..2 043 cc).

### 4.5 Notes on the numbers

The split points (for example what belongs to "before the PER(0x78) write" and what to the previous
channel) are conventions; only sums between two writes or between a write and a wait are observable,
and this set reproduces every write. Typical sizes: a non-row tick writes from about 200 cc to
1 300 cc after `E`; a row tick with four notes ends about 12 000 cc after `E`.
