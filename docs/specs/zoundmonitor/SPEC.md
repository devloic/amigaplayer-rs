# ZoundMonitor (Amiga, `sng.*`) replayer: behaviour spec

*Sanitised for publication: behaviour only.*

ZoundMonitor is a 1988 four-voice pattern editor by AJ of Activas (the replayer comes from the demo
"The Musical FruitBasket"). A song file holds the song, tracks, patterns and up to 15 sample
descriptions; the sample data lives in separate files in a `Samples/` directory next to the song.
This spec describes the observable behaviour of the reference (the Wanted Team Eagleplayer
"ZoundMonitor" run on the emulated machine): file format, detection, replay state machine, the exact
Paula register writes and their colour-clock timing. Everything below was checked against
register-write logs of the reference (see LOGS.md); numbers are big-endian, bytes unsigned unless
said otherwise, "signed byte" means two's complement.

## 1. Files

### 1.1 Song file layout

| offset | size | meaning |
|---|---|---|
| 0 | 1 | `T` = number of song positions minus 1 |
| 1 | 1 | `P` = number of patterns minus 1 |
| 2 | 1 | start position |
| 3 | 1 | end position (exclusive: the song wraps when the position reaches it) |
| 4 | 1 | speed (ticks per row) |
| 5 | 16 x 54 | sample slots 1 to 16 (1.2) |
| 869 | (T+1) x 16 | position table (1.3) |
| 869 + 16(T+1) | (P+1) x 128 | patterns (1.4) |
| after | any | trailer (normally the editor's sample path, e.g. `df0:...`), see 1.5 |

### 1.2 Sample slot (54 bytes)

| offset | size | meaning |
|---|---|---|
| 0 | 4 | ignored on load (replaced by the sample's memory address, 2.1) |
| 4 | 40 | file name, zero-terminated; empty (first byte 0) = no sample |
| 44 | 1 | volume, signed byte |
| 45 | 1 | unused |
| 46 | 2 | length in words (one-shot part, written to AUDxLEN on a note start) |
| 48 | 2 | repeat length in words |
| 50 | 2 | repeat start, in words from the sample start |
| 52 | 2 | unused |

Instrument numbers 1 to 15 address slots 1 to 15. Slot 16 is loaded as a file if named but never
gets its address patched (2.1), so it is practically unusable.

### 1.3 Position table

Each position is 16 bytes: four 4-byte voice entries, voice 0 first. A voice entry is:

| byte | meaning |
|---|---|
| 0 | pattern number |
| 1 | volume offset, signed byte |
| 2 | instrument offset (added modulo 256) |
| 3 | note transpose (added modulo 256) |

### 1.4 Patterns

A pattern is single-voice: 32 rows of one 32-bit word each (128 bytes). Patterns are stored back to
back. Row word bits:

| bits | meaning |
|---|---|
| 31 | "legato": do not restart the sample if the instrument equals the last restarted one (4.3 step 8) |
| 30 | unused |
| 29..24 | note 1..63 (0 = no note on this row) |
| 23..20 | instrument 1..15 (0 = keep) |
| 19 | flag N: do not add the position's instrument offset |
| 18 | flag T: do not add the position's transpose |
| 17 | flag B: with flag A: portamento; without: pitch slide |
| 16 | flag A: with flag B: portamento; without: arpeggio |
| 15..8 | volume offset, signed byte |
| 7..0 | effect data |

Effect selection from flags A and B: none (0), arpeggio (A), slide (B), portamento (A+B).

The player reads the row word that follows a row in memory for portamento (4.3 step 4). For row 31
that is row 0 of the next pattern number; for row 31 of the last pattern it is a word made of the
file bytes at offsets `E-5 .. E-2`, where `E` = 869 + 16(T+1) + 128(P+1) is the end of the pattern
area (the player moves all song data 5 bytes towards the file start in place, so the 5 bytes after
the moved data are the old last 5 bytes).

### 1.5 Detection

Let `E` be as above. The file is ZoundMonitor when:

- `E` < file size (strictly), and
- the bytes at `E` read either `d`, `f`, any byte, `:` (offsets E..E+3), or a byte other than `d`
  followed by `amp` (offsets E+1..E+3). A `d` at E that is not followed by `f` fails.

There is no other check. The Eagleplayer's prefix is `sng.`.

### 1.6 Sample files

For each slot 1 to 16 whose name is non-empty, the file `Samples/<name>` (relative to the song
file's directory) is loaded; a missing file makes the module fail to load. Loaded files are
numbered in slot order, skipping empty slots. For slots 1 to 15 with a name:

- If the file starts with `FORM`, the sample data starts at file offset 104, and the slot's length
  (offset 46) is replaced by the 32-bit value at file offset 100 divided by 2 (kept to 16 bits). No
  other IFF parsing is done: this assumes a fixed header layout (BODY size at 100).
- Otherwise the data starts at file offset 0 and the slot's length is used as stored.
- The first word (2 bytes) of the sample data is set to zero in memory.
- The slot's address field becomes the address of the data.

Repeat length and repeat start are always taken from the slot. Slots without a name keep the raw
address field of the file (normally 0) and their stored lengths; an instrument selecting such a
slot plays from that address.

### 1.7 Song information

No title or author field. Sample names are in the slots. The host's info reports
`length = end position + 1`, `patterns = P + 1`, samples = number of named slots among 1 to 15, and
a duration of `end * 32 * speed / 50 + 1` seconds (integer division; this ignores the start
position, so it is too long when the start is not 0). One subsong. The real loop length is
`(end - start) * 32 * speed` ticks when `end > start`.

## 2. Initialisation

### 2.1 Load

The sample addresses are patched as in 1.6. The position, pattern and row state:

- position = start position, row = 0, tick counter = speed - 1 (8-bit);
- DMA-pending mask = 0;
- four voice records cleared, voice `v` gets its DMA bit `1 << v`.

No register is written at initialisation. The first tick is therefore a row tick.

### 2.2 Voice record

Each voice keeps (all start 0):

| field | size | meaning |
|---|---|---|
| vol | 16 | current volume |
| delta | 16 signed | slide step |
| per | 16 | base period |
| bit | 16 | DMA bit (1, 2, 4, 8) |
| len | 16 | length in words of the instrument |
| rlen | 16 | repeat length |
| start | 32 | sample start address |
| rstart | 32 | repeat start address |
| note | 8 | current note (after transpose) |
| flags | 8 | the 4 flag bits of the last note row |
| arp | 3 x 8 | arpeggio offsets: [0, x, y]; the first is always 0 |
| ivol | 16 signed | volume of the current instrument |
| pend | 1 | portamento pending (the next row is consumed) |
| cnt | 16 | effect counter (arpeggio and portamento) |
| pdelta | 16 signed | portamento period distance |
| pdur | 16 | portamento duration in ticks |
| last | 8 | instrument of the last sample restart under legato |
| cur | 8 | current instrument (offset applied) |

Its memory image, needed only for out-of-range note indices (3.1), is 20 words in this order:
vol, delta, per, bit, len, rlen, start (2 words), rstart (2 words), note<<8|flags, 0<<8|x, y<<8|0,
ivol, 0<<8|pend, cnt, pdelta, pdur, last<<8|cur, 0.

## 3. Tables

### 3.1 Period table

Index 0..36:

0, 856, 808, 762, 720, 678, 640, 604, 570, 538, 508, 480, 453, 428, 404, 381, 360, 339, 320, 302,
285, 269, 254, 240, 226, 214, 202, 190, 180, 170, 160, 151, 143, 135, 127, 120, 113.

A note index is an 8-bit value (note plus transpose, or plus an arpeggio offset, modulo 256). The
player does not range-check it; indices past 36 read the player's state that follows the table, as
16-bit words, at the moment of the read:

| index | word |
|---|---|
| 37 | `0x02 << 8 \| speed` |
| 38 | 15 |
| 39 | `P` (pattern count minus 1) |
| 40 | `T << 8 \| tick counter` (tick counter is 0 during a row tick, else the current count) |
| 41 | `row << 8 \| position` (current values, before this row's advance) |
| 42, 43 | `start << 8 \| end` |
| 44 | the DMA-pending mask (with the bits already set earlier in this tick) |
| 45 + 20v + k | word `k` of voice `v`'s record image (2.2), current values |
| 125 and above | memory after the player: undefined |

Index 63 (0x3F) as a note after transpose means note off (4.3 step 9).

## 4. The tick

The host calls the player at a fixed rate (every 70 940 colour clocks in the reference). On each
call: tick counter += 1 (8-bit). If it equals the speed, the tick is a **row tick** (4.3); otherwise
an **effect tick** (4.2). Speed 1 makes every tick a row tick (then DMA is never switched on and
the song is silent, 4.2). Speed 0 gives a row tick every 256 ticks.

### 4.1 Per-voice effect step ("EFFECT(v)")

Done for every voice on effect ticks, and on row ticks for voices without a new note (4.3).
By the voice's flags (from its last note row):

- **Arpeggio** (A only): cnt += 1. If cnt is even: index = (cnt / 2) mod 3, period =
  table[(note + arp[index]) mod 256], write AUDxPER. Odd: no period write. With cnt reset to 0 at the
  note, the sequence is: tick 1 nothing, tick 2 note+x, tick 4 note+y, tick 6 note, tick 8 note+x...
  (the row tick wrote the base note).
- **Portamento** (A+B): if cnt != pdur: cnt += 1, period = per + trunc(pdelta * cnt / pdur) (signed
  16-bit operands, quotient truncated toward zero, `pdur` read as signed 16-bit), write AUDxPER. At
  cnt == pdur nothing happens (the period stays where the last write left it; `per` itself is not
  changed).
- **Slide** (B only): per = per + delta (16-bit wrap, no clamp), write AUDxPER with the new per.
- **None**: no period write.

Then, always: AUDxLC = rstart, AUDxLEN = rlen.

### 4.2 Effect tick

1. If the DMA-pending mask is non-zero: write DMACON = mask | 0x8200, then mask = 0.
2. Wait for nine raster-line changes (timing in 5.4).
3. EFFECT(v) for voices 0, 1, 2, 3.

### 4.3 Row tick

Tick counter = 0. Let `pos` be the current position. For each voice v = 0..3 with its position entry
`(pat, voff, ioff, tr)` = bytes of position `pos`, voice `v`:

1. If `pend` is set: clear it, run EFFECT(v), next voice (the row is consumed, nothing read).
2. Read the row word `R` = pattern `pat`, current row; and the following word `R2` (1.4).
3. note = bits 29..24. If 0: EFFECT(v), next voice.
4. Note start. voice.note = note; voice.flags = bits 19..16. If not flag T: note = (note + tr) mod 256.
   per = table[note].
   - Arpeggio (A only): arp[1] = bits 7..4, arp[2] = bits 3..0, cnt = 0.
   - Portamento (A+B): pend = 1; target = top byte of R2 (bits 31..24), plus `tr` modulo 256
     unless bit 18 of R2 is set, then masked to its low 6 bits; pdelta = table[target] - per
     (16-bit); cnt = 0; pdur = (data byte) * speed (16-bit). The consumed next row's other fields
     are ignored.
   - Slide (B only): delta = data byte as signed.
   - None: nothing.
5. Instrument: ins = bits 23..20. If ins != 0 and not flag N: ins = (ins + ioff) mod 256.
6. If ins != 0 and ins != cur: cur = ins; load slot `ins`: ivol = slot volume (signed); then volume
   (step 7); then start = slot address, len = slot length, rlen = slot repeat length,
   rstart = start + 2 * repeat start. Otherwise (ins 0 after the offset, or same as cur) only step 7.
   An instrument of 16 or more after the offset reads past the slots: undefined.
7. Volume: vol = ivol + (row bits 15..8, signed) + voff (signed); if 0 <= vol < 64 keep, if negative
   0, else 64.
8. Restart decision: if bit 31 of R is set and cur == last, skip to step 10 (no restart). If bit 31
   is set and cur != last: last = cur, restart. If bit 31 is clear: restart (last unchanged).
9. Restart: write DMACON = bit (that voice off). If note == 63: next voice (nothing else written;
   the voice stays off until another restart). Else: mask |= bit; write AUDxLC = start,
   AUDxLEN = len.
10. Write AUDxPER = per, then AUDxVOL = vol (vol & 0x7F scaled by the host volume, see 6).

After the four voices: row += 1; if row == 32: row = 0, position += 1 (8-bit); if position == end:
position = start and the song end is signalled (playback continues from the start position).

Note that the sample started on a row tick is switched on by the next effect tick (4.2 step 1), so
it plays from `start/len` for the time until the next effect tick, after which every effect tick
rewrites LC/LEN with the repeat values (Paula takes them at the end of the current block).

## 5. Timing

### 5.1 Origin and clock model

Times are colour clocks (cc). `E` is the clock at which the host enters the player's interrupt
routine for a tick (consecutive ticks are exactly 70 940 cc apart; the first `E` is host business,
92 830 to 94 378 cc in the reference logs). The player is modelled with a CPU-cycle counter `T`
(2 cycles = 1 cc) starting at **309** at `E`. Each step below adds its cycles to `T`; a register
write is stamped `E + floor(T / 2)` with `T` *including* the write's own cost (the write cost is
the last number in its step). Both words of an AUDxLC write get the same stamp. All costs are
constant (they do not depend on data). Channel-dependent helper costs use `c` = voice number and
`CH(c) = 0, 31, 62, 70`.

Helper blocks (they belong to the host interface the reference uses; each is a fixed cost):

- `PERH(c) = 79 + CH(c)` (after every period write)
- `VOLH(c)`: the AUDxVOL write lands `155, 172, 179, 206` cycles (c = 0..3) after the block starts,
  then 28 more
- `VOLS(c) = 79 + CH(c)` (after the volume write block)
- `TWOH(c) = 115 + CH(c)` (after a note start's LEN write)

### 5.2 EFFECT(v) costs

| path | cycles |
|---|---|
| entry | 25 |
| flags A set: | 25, then: |
| arpeggio | 43; if cnt even: 204, PER write 22, then 44 + PERH(c) + 22 |
| portamento | 33; if cnt != pdur: 244, PER write 12, then 34 + PERH(c) + 22 |
| flags A clear: | 25, then: |
| slide | 36, PER write 12, then 34 + PERH(c) + 12 |
| none | 0 |
| end (always) | LC write 24, LEN write 20, then 16 |

### 5.3 Row tick costs

Start of the row tick: 102. Per voice:

| step | cycles |
|---|---|
| voice head | 39 + 25 |
| pending portamento | 38, EFFECT(v), 10, voice tail |
| row fetch | 167 |
| no note | 18, EFFECT(v), 10, voice tail |
| note: flags | 89; + 28 if flag T is clear |
| period lookup | 54 + 25 |
| effect set-up, flags A set | 25 + (arpeggio 80, or portamento 63 + 12 if bit 18 of R2 is clear + 182) |
| effect set-up, flags A clear | 25 + (28 if slide) |
| instrument field | 53; if ins field != 0: 25, + 12 if flag N clear, + 9; if then ins != 0: + 21 |
| instrument load (step 6 load) | 130 + 18, VOLUME, 126 |
| no load | 18, VOLUME, 10 |
| VOLUME | 89, then 24 if in range, 13 + 18 + 24 if clamped to 64, 13 + 4 + 24 if clamped to 0 |
| restart decision | 19; if bit 31 set: 33, + 12 if cur != last |
| restart | DMACON write 24, then 25; note 63: voice tail; else 28, LC write 24, LEN write 20, 18 + TWOH(c) |
| period and volume | PER write 20, 42 + PERH(c) + 8 + 18, VOLH(c) (with the VOL write), 18 + VOLS(c) + 12 |
| voice tail | 56 |

Nothing is written after the last voice of a row tick.

### 5.4 Effect tick costs and the raster wait

| step | cycles |
|---|---|
| mask test | 21 |
| mask non-zero | 36, DMACON write (included in the 36), then 16 |
| before the wait | 30 + 28 |
| wait | below |
| after the wait | 44 + 16 |
| per voice | 25 + 30, EFFECT(v), 48 |

The wait samples the raster line 9 times in a row; the line of a sample at `T` is
`floor((E + floor(T / 2)) / 227)` (line boundaries are at multiples of 227 cc of the absolute
clock). Repeat 9 times: `T += 16`, take the line `L`; `T += 16`, sample; while the sample equals
`L`: `T += 25`, sample again; then `T += 19`.

### 5.5 Worked check

First row tick of `basic` (E = 92 830; voice 0 loads instrument 1 at full volume, no effect):
DMACON at E+722, LC at E+760, LEN at E+770, PER at E+847, VOL at E+998, voice 1's DMACON at
E+1611. The next tick (effect tick, E = 163 770) writes DMACON 0x8207 at E+183, and voice 0's
LC/LEN after the wait at E+2281 / E+2291.

## 6. Host-related behaviours in the output

- The AUDxVOL value is `(vol & 0x7F) * hostvol / 64` per side (voices 0 and 3 use the left volume,
  1 and 2 the right), and 0 for a voice muted by the host. With the default host volume (64) the
  value is `vol`. All logs use the defaults.
- On stop the host's end routine writes DMACON = 0x000F and zero to the four AUDxVOL registers.
- The song-end signal is raised on the wrap; playback continues from the start position.
- If `end <= start` the position counts up through 255, wraps to 0 and continues until it equals
  `end`; positions past `T` read the following file data as position entries (not covered by logs).
