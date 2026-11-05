# Kim Christensen (Amiga, `KIM.*`) replayer: behaviour spec

*Sanitised for publication: behaviour only.*

Scope: modules played by the UADE/Eagleplayer player "Kim Christensen" (UADE file name
`Kim_Christensen`, prefix `KIM.`; the driver comes from the game Rotor, 1989). One subsong.

This is a behaviour description derived from the reference's register-write logs and execution
traces. It contains no code of the original. All timing is in colour clocks ("cc") or in half colour
clocks ("u", 1 u = 0.5 cc).

Status (all checked by a private model that follows only this text, against the reference):

| item | result |
|---|---|
| 7 synthetic modules used to measure the timing constants, 56 414 register writes | value, order and absolute colour clock of every write equal the reference |
| 4 further synthetic modules built after the constants were fixed (held out), 48 482 writes | equal, including the speed edge cases 0x80 and 0x90 |
| real modules | none available (see README) |

## 1. File format

### 1.1 What a module is

A module is a raw memory image cut out of a game: driver code, the driver's variables and tables,
song data and sample data, with no header. The image was taken from a fixed address in the game's
memory ("origin"). The song data pointers inside it are absolute addresses in that original memory,
so they must be converted to file offsets: `file_offset = address - origin`.

The Eagleplayer does not run the driver code inside the module. It has its own copy of the driver and
only takes data from the module. An open replayer does the same; the code bytes of the module are only
used for detection and for locating the data.

### 1.2 Locating the data (signature words)

All values are 16-bit big-endian words at even file offsets. "Long" = 32-bit big-endian.

1. From offset 0, find the first word `0x207C`. The long right after it is `A_pos` (address of the
   position list).
2. From the offset after that long, find the next word `0x0680`. The long after it is `A_pat`
   (address of the pattern data).
3. From the offset after that long, find the next word `0xE341`. The long immediately *before*
   this word is `A_per` (address of the period table in the original memory).
4. From the offset after the `0xE341` word, find the next word `0x227C`. The long after it is
   `A_ins` (address of the instrument records).
5. From the offset after that long, find the next word `0x0680`. The long after it is `A_smp`
   (address of the sample list).
6. From the offset after that long, find the next word `0x0087`. The period table starts at the
   offset of this word minus 2 (its first entry is 0, its second 0x0087); call that offset `T`.

`origin = A_per - T`. Then `pos = A_pos - origin`, `pat = A_pat - origin`, `ins = A_ins - origin`,
`smp = A_smp - origin` are file offsets.

### 1.3 Detection

The Eagleplayer accepts a file when all of these hold:

- file size >= 1800 bytes;
- the word `0x207C` occurs among the first 100 words (offsets 0, 2, ..., 198);
- after it, the words `0x0680`, `0xE341`, `0x227C`, `0x0680`, `0x0087` are found in this order by a
  word-by-word scan that starts at the word after the `0x207C` and continues from the word after
  each match. The scan here does not skip the longs that follow the signature words (unlike 1.2).
  At most 800 non-matching words may be examined in total over the five searches (a match does
  not count). If the budget runs out, reject.

After detection, initialisation can still fail ("module too short"):

- let `first = long at smp` (the first sample address). Fail if it is 0 or has bit 31 set.
- number of samples `nsmp = (first - A_smp) / 6` (the sample data follows the list directly);
  the division is unsigned 32 by 16 bits and only the low 16 bits of the quotient are used.
- walk the `nsmp` entries in order with `end = 0`: for an entry (address `a`, length `l`), if
  `end > a` (signed compare) leave `end` unchanged, otherwise `end = a + l`. Fail if
  `end - origin > file size`. (The length is added as a byte count here, even though playback uses it
  as words; this only matters for this check.)

### 1.4 Song header and position list

The long at file offset `pos - 4` is the song header. If it is 0, use `0x02000077` instead.
Header bytes: `speed` (byte 0), unused (byte 1), `repeat` (byte 2), `last` (byte 3).

- `speed`: tick counter reload value (section 3).
- `repeat`: the position where playing starts, and where it continues after the end.
- `last`: the last position index. The song has `last + 1` positions.

The position list is bytes at `pos + i`. A value is a pattern number, except `0xFF`, which is
skipped (see 3.2).

The 4 bytes at `pos - 8` serve as the "empty sample": address `module_base + pos - 8`, length
2 words (Section 6). They are not checked to be zero.

### 1.5 Patterns

Pattern `p` is at `pat + p * 132`: 4 unused bytes, then 4 channel blocks of 32 bytes. A channel block has
16 rows of 2 bytes: `note`, `instrument`. Row `r` of channel `c` is at `pat + p*132 + 4 + c*32 + r*2`.
`note = 0` means nothing happens on that channel in that row (the instrument byte is ignored).

### 1.6 Instruments

Instrument `i` is a 32-byte record at `ins + i * 32`:

| bytes | meaning |
|---|---|
| 0..7 | volume envelope: 4 segments of (target, duration), bytes |
| 8..13 | pitch envelope: 2 segments of (delay, duration, semitone offset + 24), bytes |
| 14 | vibrato delay |
| 15 | vibrato length |
| 16 | vibrato step + 99 |
| 17 | unused |
| 18 | loop flag (non-zero: loop) |
| 19 | sample number |
| 20..23 | start offset (long, bytes, added to the sample address) |
| 24..25 | length in words |
| 26..29 | repeat offset (long, bytes, added to the sample address) |
| 30..31 | repeat length in words |

### 1.7 Sample list

Entry `s` (6 bytes) at `smp + s * 6`: long address (original memory) and 16-bit length. The sample
address in the loaded module is `module_base + address - origin`. The length is used only by the
check in 1.3 and by the host's sample info.

### 1.8 Period table

The driver's period table has 43 entries, indexed by note:

```
  0: 0
  1: 0x087 0x08F 0x097 0x0A0 0x0AA 0x0B4 0x0BE 0x0CA 0x0D6 0x0E2 0x0F0 0x0FE
 13: 0x10D 0x11D 0x12E 0x140 0x153 0x168 0x17D 0x194 0x1AC 0x1C5 0x1E0 0x1FC
 25: 0x21A 0x23A 0x25C 0x280 0x2A8 0x2D0 0x30E 0x328 0x358 0x38A 0x3C0 0x3F8
 37: 0x434 0x474 0x4B8 0x500 0x550 0x5A0
```

(Higher notes have *longer* periods, so they sound lower; entry 31 is not monotonic. These are the
values the reference uses. A module contains a copy of the table at `T`, but the reference ignores
that copy.)

Indices 43 to 54 read the driver's state words that follow the table, in this order (all 16-bit,
"hi"/"lo" = upper/lower half of a 32-bit value):

| index | value |
|---|---|
| 43, 44 | hi, lo of the empty-sample address (`module_base + pos - 8`) |
| 45 | `speed << 8 | header byte 1` |
| 46 | `repeat << 8 | last` |
| 47, 48 | hi, lo of `module_base + pos` |
| 49, 50 | hi, lo of `module_base + pat` |
| 51, 52 | hi, lo of `module_base + ins` |
| 53, 54 | hi, lo of `module_base + smp` |

(header bytes after the zero-header substitution of 1.4). Indices 55 to 255 read memory beyond the
driver and are unspecified (README).

### 1.9 Info

- Subsongs: 1. No title or author field; the player reports author "Kim Christensen".
- The Eagleplayer's length is `last + 1` positions, and its duration is
  `(speed + 1) * (last + 1) * 16` ticks at 50 Hz. This estimate ignores `0xFF` entries, `repeat` and speeds
  of 0x81 and above.

## 2. State

Global: `pos` (byte), `row` (byte, 0..15), `counter` (byte), `dma` (16-bit DMA mask word), `loopmask`
(16-bit).

Per channel `c` (0..3):

- `start` (32-bit address), `len` (16), `rstart` (32), `rlen` (16)
- `vol` (16-bit, 8.8 fixed point), `per` (16-bit, 12.4 fixed point)
- volume envelope: 4 x (`count` byte, `delta` 16-bit)
- pitch envelope: 2 x (`delay` byte, `count` byte, `delta` 16-bit)
- vibrato: `vdelay` byte, `vcount` byte, `vreload` byte, `vstep` 16-bit

All 16-bit arithmetic wraps.

Init (start of playing): every field above is 0, except `pos = repeat`, `row = 0`. On a fresh load,
`counter`, `dma` and `loopmask` are also 0. The reference's re-init does not clear these three (only
relevant for a restart in the same process).

## 3. Per tick

The player is called once per tick. Tick spacing in the reference host: 70 940 cc (CIA timer, about
50 Hz). Each tick runs these steps in order:

### 3.1 Counter

`counter = (counter - 1) & 0xFF`. If the result has bit 7 set (i.e. it is >= 0x80 as a byte), this
is a **row tick** (3.2); otherwise skip to 3.4.

Consequences: on a fresh load the first tick is a row tick. With `speed` <= 0x7F a row comes every
`speed + 1` ticks. With `speed = 0x80` the counter counts down 0x80 -> 0x7F ... 0 -> 0xFF, so 129
ticks per row (except the first). With `speed` >= 0x81 every tick is a row tick.

### 3.2 Row tick: fetch

1. `dma = 0`.
2. `p = position[pos]`. While `p == 0xFF`: `pos = pos + 1`, `p = position[pos]`.
   (The skip changes `pos` permanently.)
3. `counter = speed`.
4. Remember the row address (pattern `p`, current `row`).
5. `row = row + 1`. If `row == 16`: `row = 0`, `pos = (pos + 1) & 0xFF`, and if `pos > last`
   (unsigned): `pos = repeat` and **song end** is signalled (once per wrap; the song keeps playing).
6. `loopmask = 0`.
7. For each channel `c = 0..3` in order: if the row's `note != 0`, start the note (3.3) and set
   bit `c` in `dma`.
8. Write DMACON = `dma` (bit 15 clear: this stops the channels that got a note). This write
   happens on every row tick, with value 0 if no channel got a note.

### 3.3 Note start on channel `c` (note `n`, instrument record `I`, sample entry `S = I[19]`)

- `per = table[n] << 4` (16-bit).
- if `I[18] != 0`: set bit `c` in `loopmask`.
- `a = module_base + S.address - origin`; `start = a + I.start` (32-bit), `rstart = a + I.repeat`,
  `len = I.length`, `rlen = I.repeatlength`; `vol = 0`.
- Volume envelope: with `prev = 0`, for k = 0..3: `target = I[2k]`, `dur = I[2k+1]`;
  `count[k] = dur`; `delta[k] = DIV((target - prev) * 256, dur)`; `prev = target`.
- Pitch envelope: with `m = n`, for k = 0..1: `delay[k] = I[8+3k]`, `count[k] = I[9+3k]`;
  `t0 = table[m]`; `m = (m + I[10+3k] - 24) & 0xFF`; `t1 = table[m]`;
  `delta[k] = DIV((t1 - t0) * 16, count[k])`. The second segment starts from the note the first segment
  reached.
- Vibrato: `vdelay = I[14]`, `vcount = I[15] >> 1`, `vreload = I[15]`, `vstep = (I[16] - 99)` (16-bit).
- Writes (in this order): AUDxLC = `start` (32-bit, logged as two words at the same clock),
  AUDxLEN = `len`, AUDxVOL = 0, AUDxPER = `(per as signed 16) >> 4` (equals `table[n]` for
  notes 1..42; differs when an index of 1.8 makes the shift wrap).

Table values are unsigned 16-bit; differences are taken as 32-bit signed numbers. `DIV(x, d)` (`d` is an unsigned byte): if `d == 0`, 0. Otherwise the signed quotient of the 32-bit `x` by `d`, truncated toward
zero. If that quotient does not fit in a signed 16-bit value, the result is the low 16 bits of `x`
itself (the 68000 leaves the dividend unchanged on overflow). Example: target 255, duration 1 gives
0xFF00 (as a 16-bit value, -256). An out-of-range pitch index (1.8) can produce such an overflow too.

### 3.4 Envelopes (every tick, after 3.2/3.3)

For each channel `c = 0..3`, in this order:

1. Volume: the first segment k (0..3) with `count[k] != 0`: `count[k] -= 1`, `vol += delta[k]`.
   None: nothing.
2. Pitch, first matching case only:
   - `delay[0] != 0`: `delay[0] -= 1`
   - else `count[0] != 0`: `count[0] -= 1`, `per += delta[0]`
   - else `delay[1] != 0`: `delay[1] -= 1`
   - else `count[1] != 0`: `count[1] -= 1`, `per += delta[1]`
   - else nothing
3. Vibrato, first matching case only:
   - `vdelay != 0`: `vdelay -= 1`
   - else `vcount != 0`: `vcount -= 1`, `per += vstep`
   - else `vcount = vreload`, `vstep = -vstep` (no change to `per` on this tick)

The pitch and vibrato changes accumulate in `per`; nothing restores the note's base period. On the
note's own row tick the envelopes run once right after the note start.

Then, for `c = 0..3`: AUDxVOL = `((vol as signed 16) >> 8) & 0x3FF`, AUDxPER = `(per as signed 16) >> 4`
as a 16-bit value. (The volume path multiplies by the host volume 64 and divides by 64 in 16 bits,
which leaves the low 10 bits; a negative `vol` gives 0x3FF, a volume above 64 is written as it is.)

### 3.5 DMA on and repeat (every tick)

1. `dma = dma | 0x8000`; write DMACON = `dma` (every tick, also on non-row ticks; `dma` keeps the
   channels of the last row tick until the next row tick clears it).
2. For `c = 0..3` with bit `c` of `dma` set: if bit `c` of `loopmask` is set, AUDxLC = `rstart`,
   AUDxLEN = `rlen`; otherwise AUDxLC = empty-sample address (1.4), AUDxLEN = 2. These writes repeat
   on every tick until the next row tick.

### 3.6 Write order summary

Row tick: [per started channel: LC, LEN, VOL, PER], DMACON (off), [per channel 0..3: VOL, PER],
DMACON (on), [per channel in `dma`: LC, LEN]. Non-row tick: the same without the first two groups.

### 3.7 Song end

Signalled in step 3.2.5 (the tick that reads the last row of position `last`). The reference host
stops its offline render one tick after the signalling tick. Playing continues at `repeat`. All
channel state carries over.

## 4. Timing

All times are measured from the tick's start `E` (the moment the host enters the player). A write at
`u` half colour clocks after `E` is at colour clock `E + floor(u / 2)`. In the reference host,
`E(k) = E0 + 70 940 * k` for tick k. `E0` depends on the host (LOGS.md lists it per log).

The beam is at line `floor(clock / 227)`. Only line changes matter (section 4.4).

### 4.1 Row tick, before the envelopes

`u = 678 + 69 * (number of 0xFF entries skipped, one per increment of `pos`) + 117 * (row wrapped to 0)
+ 178 * (song end signalled)`. This is the start of channel 0. The song-end term includes the host's
song-end notification.

For each channel `c = 0..3`:

- no note: `u += 57`.
- note: `u += 3619 - 138 * Z_vol - 138 * Z_pitch + 20 * L`, where `Z_vol` is the number of volume segments
  with duration 0 (0..4), `Z_pitch` the number of pitch segments with duration 0 (0..2), `L` = 1 if the loop
  flag is set. Then the writes:
  - LC at `u`; `u += 20`
  - LEN at `u`; `u += LV[c]`
  - VOL at `u`; `u += VP[c]`
  - PER at `u`; `u += PE[c]`

| c | LV | VP | PE |
|---|---|---|---|
| 0 | 177 | 163 | 236 |
| 1 | 194 | 194 | 267 |
| 2 | 201 | 225 | 298 |
| 3 | 228 | 233 | 306 |

After channel 3: `u += 27`, DMACON (off) at `u`; `u += 35`. This is the envelope start.

### 4.2 Non-row tick

The envelope start is `u = 328`.

### 4.3 Envelopes and their writes

First, for each channel `c = 0..3`, `u += Cvol + Cpitch + Cvib` (all four channels' costs come before any of the writes below), with the path the channel takes on this
tick (3.4):

| volume path | Cvol | pitch path | Cpitch | vibrato path | Cvib |
|---|---|---|---|---|---|
| segment 0 active | 0 | first delay | 0 | delay | 430 |
| segment 1 active | 27 | first segment | 39 | step | 469 |
| segment 2 active | 50 | second delay | 50 | reload | 467 |
| segment 3 active | 63 | second segment | 75 | | |
| none | 35 | none | 47 | | |

Then for `c = 0..3`: VOL at `u`; `u += EVP[c]`; PER at `u`; and for `c < 3`, `u += EPN[c]`.

| c | EVP | EPN |
|---|---|---|
| 0 | 151 | 262 |
| 1 | 182 | 269 |
| 2 | 213 | 296 |
| 3 | 221 | - |

### 4.4 The two raster waits

The player twice waits for 9 raster-line changes by polling the beam's line. One wait, started at `u`:

```
r1 = u
repeat 9 times:
    L = line at r1
    r = r1 + 16
    while line at r == L: r = r + 25
    (r is the poll that saw the change)
    r1 = r + 35            (not after the 9th change)
the wait ends at r (the 9th change)
```

"line at x" is `floor((E + floor(x / 2)) / 227)`.

Sequence after the last envelope PER write at `u`:

1. Wait 1 starts at `u + 120`; it ends at `r`.
2. DMACON (on) at `r + 133`.
3. Wait 2 starts at DMACON + 62; it ends at `r'`.
4. `u = r' + 146`. For `c = 0..3`:
   - channel not in `dma`: `u += 47`
   - loop channel: `u += 47`; LC at `u`; `u += 20`; LEN at `u`; `u += 41`
   - empty-sample channel: `u += 47`; LC at `u`; `u += 16`; LEN at `u`; `u += 51`

### 4.5 Notes on the numbers

The split points between costs are conventions (for example how much of a note start is "before
LC"). Only the sums are observable, and the set above reproduces every write. Typical sizes: a
non-row tick writes from about 1 240 cc to 6 400 cc after `E`. In the logs the latest write is about
14 900 cc after `E` (a row tick with four notes and slow raster phases).
