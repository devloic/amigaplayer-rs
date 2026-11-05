# Nick Pelling Packer: behaviour spec

*Sanitised for publication: behaviour only.*

Scope: the "Nick Pelling Packer" music format (Nick Pelling, 1992-93; used in Mindscape's Battletoads
among others) as played by the reference: the UADE player "Nick_Pelling_Packer" (internal name
"Nick Pelling Packer", Wanted Team adaptation, version 1.0). A song is two files: a packed song file
(`NPP.<name>`) and a sample set (`SMP.<name>`, or a shared `SMP.set`). The replay is a ProTracker-style
4-channel pattern player with its own quirks, all listed below.

Everything here is a description of observed behaviour in my own words. A private value model written
from this text reproduces every audio register write (register, value, order) of the reference over
31 125 ticks of 20 synthetic modules; a timing model from section 9 puts 98.6% of 274 481 writes within
1 colour clock (cc) and 99.7% within 2 cc of the reference. No real module of this format was available;
see README.md.

Numbers: values are hexadecimal when written 0x..; "word" = 16-bit big-endian; times in cc
(1 cc = 2 CPU cycles).

## 1. Files, detection, loading, info

### 1.1 Song file (`NPP.*`)

| offset | size | content |
|---|---|---|
| 0 | 4 | magic `COMP` |
| 4 | long | N = offset of the song name; also the end of the pattern offset table |
| 8 | long | Q = offset of the song table |
| 12 | 4 × P | pattern offset table: long per pattern, offset of its packed data from the file start; P = N/4 − 3 |
| N | | song name, zero-terminated, then 31 sample names, each zero-terminated (empty names allowed; the reference always reads exactly 31 strings) |
| Q | 1 | song length L (positions) |
| Q+1 | L (up to 128) | position list: pattern number per position |
| ... | | packed patterns (anywhere; addressed through the offset table) |

Detection (song file): bytes 0..3 are `COMP`; word at 4 is 0; word at 6 (= N) is non-zero, at most
0x110, at least 0x10 and a multiple of 4; the long at N − 4 (the last pattern offset) is not larger than
the file size. The reference also requires the sample set (1.2), else loading fails ("could not load
additional files").

Sample set location: the file name of the song with its first four characters replaced: if the name starts
with `NPP.` (each letter in either case) the sample set is `SMP.` + the rest of the name, in the same
directory; if that file cannot be loaded, `SMP.set` in the same directory is tried.

Info: title = the song name string; one subsong; "length" = L; number of samples = number of non-empty
sample names (the reference counts names up to the song table); no author string in the files.

### 1.2 Sample set (`SMP.*`)

| offset | size | content |
|---|---|---|
| 0 | word | K = number of entries, 1..31 (0 or more than 31: "corrupt module") |
| 2 | 30 × K | entries (below) |
| 2 + 30K | | sample data of the entries, concatenated in entry order, each 2 × length bytes |

Entry (30 bytes): name (22 bytes, zero-terminated if shorter), word length (in words), byte finetune/flags,
byte volume, word repeat start (words), word repeat length (words). If the file is shorter than
2 + 30K + the sum of 2 × length, the reference refuses it ("module too short").

### 1.3 Sample slots (31 records)

At load, each of the 31 sample names of the song file (slot 1..31 in order) is resolved into a record
(address, length, repeat address, repeat length, volume, finetune):

- Empty name: everything 0 (address 0, length 0, repeat address 0, repeat length 0, volume 0, finetune 0).
- Name not found in the set: address 0, length 2, repeat address 0, repeat length 2, volume 0, finetune 0.
- Found: entries are searched in order; a name matches when it is byte-equal to the entry's name up to
  and including the terminating zero. Data address = start of the set's data area + sum of 2 × length of
  all earlier entries.
- Alias: if the matching entry has length 0, it stands for another entry: with n = its repeat-start
  word, the record is built from entry n + 1 (counting entries from 0) and its data address is the
  data-area start + sum of 2 × length of entries 1..n (entry 0's length is not counted; this is how the
  reference computes it, even though it is not entry n + 1's real data position unless entry 0 has
  length 0).
- Delta coding: if bit 7 of the entry's finetune byte is set, the sample data is delta-decoded in place
  once (each byte from the second on becomes the 8-bit sum of itself and the already decoded previous
  byte, over 2 × length bytes) and bit 7 is cleared in the set, so a second slot naming the same sample
  does not decode it again.
- Repeat: if the repeat length is less than 4 words, the first 4 bytes of the sample data are set to 0
  (this changes the data that plays) and the record gets repeat address = sample address, repeat length 2.
  Otherwise repeat address = sample address + 2 × repeat start, repeat length as given.
- Volume = min(entry volume, 64). Finetune = low nibble of the finetune byte (0..15; 8..15 are the
  negative finetunes as in ProTracker).
- Record length = the entry's length word.

## 2. Tables

- Period table P: the standard ProTracker finetune table, 16 rows of 36 periods (row f = finetune f,
  f = 0..7 then −8..−1 as rows 8..15), stored as one array of 576 words; entry index = 36 × f + note
  (note 0 = C-1 = 856 in row 0, note 35 = B-3 = 113 in row 0). Row 0 is 856 808 762 720 678 640 604 570
  538 508 480 453 428 404 381 360 339 320 302 285 269 254 240 226 214 202 190 180 170 160 151 143 135 127
  120 113.
- Note lookup: for a period p in 0..856, note(p) = the largest index i (0..35) of row 0 with P[i] ≥ p.
  (So any period, also one not in the table, maps to a note; p = 0..113 gives 35.) Periods above 856 are
  outside the reference's lookup (undefined).
- Vibrato/tremolo sine (32 bytes): 0x00 0x18 0x31 0x4A 0x61 0x78 0x8D 0xA1 0xB4 0xC5 0xD4 0xE0 0xEB 0xF4
  0xFA 0xFD 0xFF 0xFD 0xFA 0xF4 0xEB 0xE0 0xD4 0xC5 0xB4 0xA1 0x8D 0x78 0x61 0x4A 0x31 0x18.
- A 16-byte "funk" table follows the sine in the player (0x00 0x05 0x06 0x07 0x08 0x0A 0x0B 0x0D 0x10
  0x13 0x16 0x1A 0x20 0x2B 0x40 0x80); it is never used except by out-of-range reads (2.1).

### 2.1 Out-of-range period indexes

Arpeggio and glissando read P at computed indexes that can leave 0..575 (glissando walks the index, see
7.3). The reference then reads whatever lies around the table, which is defined data in this range:

- index −1..−24: the 48 bytes sine + funk taken as words, i.e. index −k is the word made of bytes
  48 − 2k and 49 − 2k of that 48-byte sequence (−1 = 0x4080, −2 = 0x202B, −3 = 0x161A, ...).
- index 576..703: the break-row table of 6.3 as words (index 576 = bytes 0 and 1 of it = 0x0001, 577 =
  0x0203, ..., 580 = 0x0809, 581 = 0xFFFF, ...).
- index 704..1132: the note lookup as bytes holding 2 × note(p) for p = 0, 1, 2, ..., 856, then one 0 byte,
  taken as words (704 = 0x4646, ...).
- anything further: undefined (player state / code).

These cases do happen in practice (a glissando with tone portamento runs off the table within a few ticks,
see 7.3).

## 3. Pattern data

A pattern is 64 rows × 4 channels × 4 bytes = 1024 bytes once unpacked. Cell bytes b0 b1 b2 b3 as in
ProTracker: sample = (b0 & 0xF0) | (b2 >> 4); note period = ((b0 << 8) | b1) & 0x0FFF; command = b2 & 0x0F;
parameter = b3.

Packing (byte stream at the pattern's offset, output into a 1024-byte buffer):
- a byte c in 0..127: copy the next c + 1 bytes of the stream to the output (literal run);
- a byte c in 128..255 followed by a byte n: if n = 0 the pattern ends; otherwise copy n bytes from the
  output starting (256 − c) bytes back from the current output end, byte by byte (the source may overlap
  the bytes being written).
Every pattern in practice unpacks to exactly 1024 bytes; longer output is undefined. The unpacking is
done each time the position changes (6.4) and once at song start, into the same buffer.

## 4. State

Per channel (0..3): stored cell (4 bytes; FF FF FF FF at start), current sample record (none at start),
period (16 bits; may carry bits above 0x0FFF, see 7.1), output period (0x7C at start), volume (0..64),
output volume (0 at start), finetune (0..15), note index (0..35, may walk out, 7.3), current index
(= note index + 36 × finetune), arpeggio step (0..2), tone-portamento target (0 = none), direction,
speed, vibrato command byte, vibrato position (8 bits), tremolo command byte, tremolo position (8 bits),
control byte (low nibble: vibrato control, high nibble: tremolo control; bits 0..1 of each = wave, bit 2 =
keep position on new notes), glissando byte (low nibble: glissando on/off, high nibble: "funk" value),
loop row, loop counter. All 0 at start unless stated.

Global: speed 6, tick counter 0, position 0, row 0, break row 0, jump flag off, loop flag off, pending
delay 0, delay 0. Slide mask: a byte that is 0 when the player is loaded (song start does not reset it),
see 7.1. DMA mask (per tick, starts at 0 every tick).

Note the first row is played on the 6th tick (the counter starts at 0 and the row is read when it reaches
the speed).

## 5. Start of the song (hardware writes before the first tick)

In this order (cc after the first write in the reference): AUD0VOL 0 (0), AUD0PER 0x7C (16), AUD1VOL 0
(83), AUD1PER 0x7C (99), AUD2VOL 0 (166), AUD2PER 0x7C (182), AUD3VOL 0 (249), AUD3PER 0x7C (265), DMACON 0x000F
(296), INTENA 0x0780 (304), INTREQ 0x0780 (312), CIA-A port A bit 1 set (logged as a write of 0x0002 to
0xBFE001) (332); then the pattern of position 0 is unpacked; then DMACON 0x000F, AUD0DAT 0 (+8),
AUD1DAT 0 (+6), AUD2DAT 0 (+6), AUD3DAT 0 (+6), INTENA 0x0780 (+8), INTREQ 0x0780 (+8). The time of the
unpack is 6372.75 + 29.5 × (literal runs) + 36 × (back references) cc from the CIA write to the next
DMACON write.

## 6. One tick

### 6.1 Order

1. DMA mask := 0. Tick counter += 1.
2. If the counter ≥ speed: counter := 0 and this is a row tick:
   - if the delay counter is non-zero (a pattern delay is repeating the row): the per-tick pass (7) on
     channels 0..3, with the tick counter 0;
   - else the row pass (6.2) on channels 0..3;
   - then the row advance (6.4).
3. Else: the per-tick pass (7) on channels 0..3; then, if the jump flag is set, a position change (6.4)
   (never happens in practice, the flag is consumed in the row tick that sets it).
4. Output phase (8).

### 6.2 Row pass, per channel

1. If the channel's stored cell (the previous row's) is all zero: output period := period.
2. Store the new cell (row × 16 + channel × 4 in the buffer).
3. If the sample number s ≠ 0: record := slot s (1..31; 32..255 undefined); volume := output volume :=
   record volume; finetune := record finetune.
4. If the note period is 0: row effect (6.3); done.
5. Command 3 or 5 with a note: tone-portamento setup: target := P[note(period) + 36 × finetune]; if
   target = period, target := 0 (none); else direction := up if target > period else down (signed
   compares). Then the row effect; done (no trigger, no writes).
6. Command 9 with a note: the row effect first (nothing for 9), then the trigger.
   Command E with x = 5 (E5y) and a note: finetune := y and current index := note index + 36y first, then
   the trigger. Any other command with a note: the trigger.

Trigger:
1. note index := note(note period); current index := note index + 36 × finetune; period := output period
   := P[current index].
2. If the command is E with x = D (EDy): no writes here; the note-delay check (7.4, ED) with the tick
   counter 0; done (no row effect).
3. Writes, in order: AUDxLC := record address (high word, low word), AUDxLEN := record length,
   AUDxPER := output period, AUDxVOL := 0, DMACON := channel bit (DMA off). DMA mask |= channel bit.
4. Arpeggio step := 0; vibrato position := 0 unless vibrato control bit 2 is set; tremolo position := 0
   unless tremolo control bit 2 is set.
5. The row effect (6.3).

A note with no record selected since the start (record none) is undefined (the reference reads a record
at address 0).

### 6.3 Row effects (tick 0 of a normal row)

| cmd | effect |
|---|---|
| 0..8, A | output period := period (4, 7 included) |
| 9 | nothing (sample offset is not implemented) |
| B | if (position + 1) & 0xFF = L: report song end. position := ((param & 0x7F) − 1) & 0xFF; break row := 0; jump flag on |
| C | volume := output volume := min(param, 64) |
| D | break row := BCD(param), jump flag on. BCD(v) = 10 × (v >> 4) + (v & 15) if both nibbles ≤ 9, else 0 |
| E | extended command, tick-0 rules (7.4) |
| F | param ≠ 0: speed := param, tick counter := 0. param = 0: report song end (nothing else changes; playing continues) |

The BCD conversion is a 256-byte table in the player: entry v = BCD(v) for valid v, 0xFF for invalid
(that value is what 2.1 reads for indexes 576..703). Break rows 64..99 are undefined (rows past the
buffer).

### 6.4 Row advance and position change

Row advance: row += 1. If the pending delay ≠ 0: delay := pending delay, pending delay := 0. If the delay
≠ 0: delay −= 1, and if it is still ≠ 0: row −= 1 (the row is repeated). If the loop flag is on: loop flag
off, row := break row, break row := 0. Then: if row ≥ 64: position change; else if the jump flag is on:
position change.

Position change: row := break row; break row := 0; jump flag off; position := (position + 1) & 0x7F; if
position ≥ L: report song end and position := 0. Unpack the pattern of the new position (3).

Song end is only a report to the host; the replay itself loops (the reference host stops playback a few
ticks after the report). Reports happen at: the wrap of 6.4, B on the last position, F00.

## 7. Per-tick effects

Run for each channel with the stored cell's command and parameter, on ticks 1..speed−1, and on every
channel in a row tick whose row is being repeated by a pattern delay (then with the tick counter 0, so
E commands use the tick-0 rules and the others act as on any other tick).

### 7.1 Commands

| cmd | effect |
|---|---|
| 0 | param = 0: output period := period. Otherwise step := step + 1, 3 becomes 0; step 1: output period := period; step 2: output period := P[current index + (param >> 4)]; step 0: P[current index + (param & 15)] |
| 1 | slide: amount := param AND slide mask; slide mask := 0xFF; p := (period & 0x0FFF) − amount, clamped to 124..856; output period := p; period := p OR (old period & 0xF000) |
| 2 | as 1 with + amount |
| 3 | if param ≠ 0: portamento speed := param and the stored cell's parameter byte := 0. Then the tone-portamento step (7.3) |
| 4 | if param ≠ 0: in the vibrato command byte, the low nibble := param's low nibble if that is ≠ 0, the high nibble := param's high nibble if that is ≠ 0. Then the vibrato step (7.2) |
| 5 | tone-portamento step (speed unchanged), then volume slide with param |
| 6 | vibrato step (command byte unchanged), then volume slide with param |
| 7 | output period := period; tremolo (7.2) |
| 8, 9, B, C, D, F | nothing |
| A | output period := period; volume slide with param |
| E | extended command (7.4), tick-0 rules if the tick counter is 0, else the other-tick rules |

Slide mask: starts at 0 when the player is loaded, so the very first 1xx/2xx slide of a run moves by 0 on
its first tick; E1y/E2y set it to 0x0F before their slide; every slide leaves it at 0xFF.

Volume slide: if param ≥ 0x10: v := volume + (param >> 4), else v := volume − (param & 15); clamped to
0..64; volume := output volume := v. (Both nibbles set: up only.)

### 7.2 Vibrato and tremolo

Wave value from a position pos (8 bits) and a wave w (0..3): k := (pos & 0x7C) >> 2; w = 0: sine[k];
w = 1: (8k) & 0xFF, replaced by 255 minus it when pos bit 7 is set; w = 2, 3: 255.

Vibrato step: w := vibrato control & 3; d := (depth × wave value) >> 7 with depth = vibrato command low
nibble; p := period ± d (minus when pos bit 7 is set; period taken as signed 16 bits); clamped to 124..856;
output period := p (period unchanged); pos := (pos + 4 × speed) & 0xFF, speed = command high nibble.

Tremolo: parameter merge into the tremolo command byte as for command 4; w := tremolo control & 3;
the wave value uses the tremolo position; d := (depth × value) >> 6; v := volume ± d (minus when tremolo
pos bit 7 is set), clamped to 0..64; output volume := v (volume unchanged); then
pos := (pos + 0xC0 + 4 × speed) & 0xFF. The extra 0xC0 is a quirk of the reference (the speed nibble is
extracted with a sign-extended word); it makes tremolo phases differ from ProTracker.

### 7.3 Tone portamento step and glissando

If target = 0: nothing. Direction up: period += speed; if target ≤ period: period := target, target := 0.
Direction down: period −= speed; if target ≥ period: period := target, target := 0 (signed 16-bit
compares). Then p := period; if the glissando nibble is 0: output period := p. Otherwise, with
c := P[current index]: p = c: keep; p > c: p := P[current index + 1] and note index, current index += 1;
p < c: if p > P[current index − 1] keep p, else p := P[current index − 1] and both indexes −= 1. Output
period := p. (This snaps toward the neighbour entry on the wrong side, so each tick of a running
portamento moves the index one entry further; it soon reads outside the table, 2.1.)

### 7.4 Extended commands Exy

Tick-0 rules:

| x | effect |
|---|---|
| 0 | filter: CIA-A port A bit 1 cleared, then set to 2 × (y & 1): two writes to 0xBFE001, logged as 0x0000, then 0x0000 or 0x0002, 6 cc apart |
| 1 | slide mask := 0x0F, then as command 1 with param (moves by y) |
| 2 | slide mask := 0x0F, then as command 2 |
| 3 | glissando nibble := y |
| 4 | vibrato control := y |
| 5 | finetune := y; current index := note index + 36y (period not recomputed) |
| 6 | y = 0: loop row := row (the row being played). y ≠ 0: if the loop counter ≠ 0: counter −= 1, and if it is now 0 nothing more happens; if the counter was 0: counter := y. Unless finished: break row := loop row, loop flag on |
| 7 | tremolo control := y |
| 8 | if the note period is 0: retrigger |
| 9 | nothing |
| A | volume := output volume := min(volume + y, 64) |
| B | volume := output volume := max(volume − y, 0) |
| C | if y = 0: volume := output volume := 0 |
| D | note-delay check |
| E | if the delay counter is 0: pending delay := y + 1 |
| F | funk nibble := y (no audible effect; costs time, 9) |

Other-tick rules: x = 8: if y ≠ 0 and tick counter mod y = 0: retrigger. x = C: if y = tick counter:
volume := output volume := 0. x = D: note-delay check. Others: nothing.

Note-delay check: if y = tick counter and the cell's first word (all 16 bits, including the sample's high
nibble) ≠ 0: retrigger.

Retrigger: AUDxLC := record address, AUDxLEN := record length, output period := period, output volume :=
volume, DMACON := channel bit (off), DMA mask |= channel bit. (No period or volume write here; the output
phase writes them.)

## 8. Output phase (every tick)

For channels 0, 1, 2, 3: AUDxVOL := output volume (scaled by the host volume, below), then AUDxPER :=
output period.

Then, if the DMA mask ≠ 0: raster wait 1 (9.3); DMACON := 0x8000 | mask; raster wait 2; then for each
channel in the mask in ascending order: AUDxLC := the channel's record repeat address (high, low word),
AUDxLEN := repeat length.

Host volume: channels 0 and 3 use the left level, 1 and 2 the right level; written volume = (output
volume × level) >> 6 with level = (balance × master volume) >> 6; at the defaults (64) the output volume
is written unchanged. A muted voice writes volume 0.

## 9. Timing

### 9.1 Ticks

Ticks come from the host timer every 70 940 cc. Each tick k has a start S_k = S_0 + 70 940 k. In the
reference the first tick's channel-0 volume write comes 71 598 cc after the last write of section 5, and
the first tick is a non-row tick with no effects, so S_0 = (last init write) + 71 175.94 cc.

All writes of a tick follow from S_k by adding costs of the paths taken, in program order (the order of
sections 6 and 7): the time of a write = time of the previous write (or S_k) + the costs of all items
passed since then. The items and their costs are in the table of 9.4 (fitted to the logs by least
squares; the original's real costs are multiples of 0.5 cc, but the split between items that always
occur together is arbitrary; use the values as given, with their decimals, and round only when writing).
Multiply and divide in effects take a fixed time in the reference (no data dependence).

### 9.2 Structure of a tick

- Non-row tick: S_k + Pnr (0); channel 0: its effect items (7); channel 1: fx1 + its items; channel 2:
  fx2 + items; channel 3: fx3 + items; then `out` to the channel-0 volume write.
- Pattern-delay row tick: S_k + Pdel; then as a non-row tick for the channels (with tick-0 rules for E);
  then the row advance items, then the end of tick (below).
- Row tick: S_k + Prow; channel 0 row-pass items; channel 1: r1 + items; channel 2: r2 + items;
  channel 3: r3 + items; then the row advance items and the end of tick.
- End of tick: no position change: `out`. Row reached 64: `adv_np`, then the unpack, then `out`.
  Jump/break flag: `jc_np`, then the unpack, then `out`. A wrap adds `np_end` + `songend`. Unpack =
  1024 × 6 + 29.5 × (literal runs) + 36 × (back references) cc.
- Per channel, the items in the order they are passed: e.g. a row channel with a note and command C:
  [rz] [rs] rn rnx tg -> LC write -> w_len -> LEN -> w_per -> PER -> w_vol -> VOL -> w_dmaN -> DMACON ->
  tg_post [tg_vk] [tg_tk] fr fr_C. An E command in the row: fr fr_E e0 e0_x [+ sub-items]; in a per-tick
  pass: cE then e0 e0_x... (tick counter 0) or e1 e1_x... (other ticks). Command 5: c5 + the tone-
  portamento items + the volume-slide items; 6: c6 + vib + wave/sign/clamp items + volume-slide items;
  4: c4 [vp vp_lo vp_hi] vib + ...; 7: c7 [mp ...] + wave/sign/clamp items. A retrigger inside an effect:
  rt -> LC -> rt_len -> LEN -> rt_dma -> DMACON, after the items that lead to it (e8_go, ed_go, or e0_8
  with no note). `funk` is added at the start of each per-tick effect, row effect and EDy trigger check of
  a channel whose funk nibble is non-zero, and once more when EFy itself sets a non-zero value.
- E0x: the first CIA write is 3.75 cc after the items up to and including e0_0 have been added in a row
  effect, 28 cc before that point when it runs in a pattern-delay row; the second 6 cc after the first.

### 9.3 Output phase and raster waits

Output phase, from the channel-0 volume write: VOL0 0, PER0 14, VOL1 86.5, PER1 100.5, VOL2 173, PER2 187,
VOL3 259.5, PER3 273.5.

Raster lines start at multiples of 227 cc (absolute). Each wait waits for 9 line changes by polling the
beam. Model in CPU cycles (x = 2 × cc), with line(x) = floor(x / 2 / 227):
- start: wait 1: x := 2 × (time of the PER3 write) + 95; wait 2: x := 2 × (time of the DMACON write) + 61;
- 9 times: l := line(x); x += 18; while line(x) = l: x += 25; and except after the 9th: x += 33.
- wait 1 ends with DMACON at x/2 + 37.5 cc;
- wait 2: the first repeat LC write at x/2 + 61 (first channel in the mask 0), 70 (1), 79 (2), 87.5 (3);
  LEN 10 cc after its LC; the next channel's LC 30.5 cc after the previous LEN, plus 8.5 cc for each
  channel skipped in between.

This reproduces the reference in 403 of 413 wait-1 cases checked directly (the other 10 detect the line
change one poll, 12.5 cc, later) and in all 413 wait-2 cases.

### 9.4 Cost table (cc)

**Tick kinds and channel overheads**

| item | meaning | cc |
|---|---|---|
| Pnr | non-row tick: tick start to the start of channel 0 (definition of the tick start S) | 0.00 |
| Prow | row tick (normal): tick start to the start of channel 0 | 60.48 |
| Pdel | row tick while a pattern delay repeats the row: tick start to the start of channel 0 | 84.12 |
| fx1 | per-tick pass: channel 1 entry | 61.14 |
| fx2 | per-tick pass: channel 2 entry | 62.46 |
| fx3 | per-tick pass: channel 3 entry | 57.20 |
| r1 | row pass: channel 1 entry | 82.64 |
| r2 | row pass: channel 2 entry | 82.23 |
| r3 | row pass: channel 3 entry | 82.50 |
| funk | extra, every time a channel's effect is handled (per-tick pass, row effect pass, note-delay trigger) while its EFx nibble is non-zero | 17.00 |

**Per-tick effects (non-row ticks and pattern-delay rows); commands 8, 9, B, C, D, F cost 0**

| item | meaning | cc |
|---|---|---|
| c0 | command 0 (arpeggio) base | 45.42 |
| arp_z | + parameter 0 | -25.05 |
| arp_b | + step 1 (base period) | 13.59 |
| arp_h | + step 2 (high nibble) | 39.59 |
| arp_w | + step counter wraps to 0 | 17.30 |
| arp_n | + step 0 (low nibble) | 17.30 |
| c1 | command 1 base | 34.52 |
| c2 | command 2 base | 34.50 |
| sl_mid | period slide (1xx, 2xx, E1x, E2x) result within 124..856 | 36.54 |
| sl_lo | slide result below 124 | 34.92 |
| sl_hi | slide result above 856 | 40.54 |
| c3 | command 3 base | 45.24 |
| tp_par | + non-zero parameter (stored, then cleared in the cell) | 14.01 |
| c5 | command 5 base (then tone portamento without parameter, then volume slide) | 44.65 |
| tp_off | tone portamento: no target | -23.27 |
| tp_u | tone portamento: moving to a higher period | 56.57 |
| tp_hit | + target reached (upward) | 23.01 |
| tp_d | tone portamento: moving to a lower period | 56.58 |
| tp_dhit | + target reached (downward) | 18.11 |
| gl_off | glissando off (store period) | -12.38 |
| gl_eq | glissando: equals the current note | 8.10 |
| gl_up | glissando: above the current note (moves one entry up) | 45.21 |
| gl_keep | glissando: below, kept | 23.01 |
| gl_dn | glissando: below, moves one entry down | 49.22 |
| c4 | command 4 base | 37.55 |
| vp | + non-zero parameter | 35.11 |
| vp_lo | + its low nibble non-zero | 6.10 |
| vp_hi | + its high nibble non-zero | 5.79 |
| c6 | command 6 base (vibrato without parameter, then volume slide) | 37.03 |
| vib | vibrato step | 74.58 |
| vw_sine | + sine wave | 23.22 |
| vw_ramp | + ramp wave | 30.17 |
| vw_rneg | + ramp, position negative | 1.95 |
| vw_sq | + square wave | 21.19 |
| vneg | + position negative (subtract) | 1.96 |
| vc_mid | + result within 124..856 | 24.04 |
| vc_lo | + result below 124 | 22.56 |
| vc_hi | + result above 856 | 27.97 |
| c7 | command 7 (tremolo) base | 131.76 |
| mp | + non-zero parameter | 34.77 |
| mp_lo | + low nibble non-zero | 6.10 |
| mp_hi | + high nibble non-zero | 6.14 |
| tw_sine | + sine wave | 19.07 |
| tw_ramp | + ramp wave | 27.93 |
| tw_rneg | + ramp, position negative | 1.99 |
| tw_sq | + square wave | 18.88 |
| tneg | + position negative | 1.99 |
| tc_mid | + result within 0..64 | 20.62 |
| tc_lo | + result below 0 | 22.65 |
| tc_hi | + result above 64 | 22.61 |
| cA | command A base | 2.81 |
| vs_up | volume slide up | 43.73 |
| vs_dn | volume slide down | 40.76 |
| vs_mid | + result within 0..64 | 26.81 |
| vs_lo | + below 0 | 28.83 |
| vs_hi | + above 64 | 28.84 |
| cE | command E base (per-tick pass) | 35.82 |
| e1 | E, tick counter not 0: dispatch | 13.12 |
| e1_0 | + x = 0..7, 9, A, B, E, F (nothing) | -2.52 |
| e1_8 | + x = 8 | 10.30 |
| e8_div | + x = 8, y non-zero (divide) | 77.87 |
| e8_go | + retrigger happens | 10.96 |
| e1_C | + x = C | 16.03 |
| ec_hit | + cut happens | 12.06 |
| e1_D | + x = D | 19.55 |
| ed_t | note delay: counter differs | -3.65 |
| ed_n | note delay: counter matches, note word 0 | 4.70 |
| ed_go | note delay: retrigger | 19.37 |

**Row pass (channel with a new cell)**

| item | meaning | cc |
|---|---|---|
| rz | + previous cell of the channel was all zero | 10.00 |
| rs | + sample number non-zero | 52.96 |
| rnn | no note: then the row effect | -9.51 |
| rn | note present | 76.38 |
| rn35 | + command 3 or 5 | 21.67 |
| ps_eq | + target equals the current period | 4.79 |
| ps_gt | + target higher | 7.48 |
| ps_lt | + target lower | 9.41 |
| rn9 | + command 9 (row effect first) | 18.80 |
| rnE | + command E | 26.10 |
| rnE5 | + E5x (finetune before the trigger) | 76.90 |
| rnx | + any other command | 9.81 |
| tg | trigger: to the location write | 54.71 |
| tg_ed | trigger with EDx: no writes, note-delay check instead | 6.61 |
| w_len | location -> length write | 10.00 |
| w_per | length -> period write | 10.00 |
| w_vol | period -> volume write | 8.00 |
| w_dma0 | volume -> DMA-off write, channel 0 | 86.22 |
| w_dma1 | channel 1 | 102.00 |
| w_dma2 | channel 2 | 117.53 |
| w_dma3 | channel 3 | 121.46 |
| tg_post | after the DMA-off write, to the row effect | 42.14 |
| tg_vk | + vibrato position kept | -7.96 |
| tg_tk | + tremolo position kept | -7.95 |

**Row effects (tick 0)**

| item | meaning | cc |
|---|---|---|
| fr | row effect base | 73.09 |
| fr_base | + commands 0..8, A (output period := period) | -13.98 |
| fr_nop | + command 9 | -23.90 |
| fr_B | + B | 44.29 |
| fr_C | + C, value <= 64 | 11.03 |
| fr_Cc | + C, value > 64 | 13.00 |
| fr_D | + D, valid BCD | 17.24 |
| fr_Dx | + D, invalid BCD | 19.73 |
| fr_E | + E | 11.82 |
| fr_F | + F, non-zero | 6.30 |
| fr_F0 | + F00 (song end report) | -12.44 |
| e0 | E, tick counter 0 (row effect, or any E in a pattern-delay row): dispatch | 34.52 |
| e0_0 | + x = 0 (filter, CIA writes) | 2.05 |
| e0_1 | + x = 1 (then slide) | 21.45 |
| e0_2 | + x = 2 (then slide) | 21.53 |
| e0_3 | + x = 3 | -1.95 |
| e0_4 | + x = 4 | -1.98 |
| e0_5 | + x = 5 | 27.12 |
| e0_6 | + x = 6 | 14.52 |
| e6_mark | + y = 0 | -17.23 |
| e6_set | + first loop jump | 17.83 |
| e6_dec | + further loop jump | 19.96 |
| e6_done | + loop finished | -6.04 |
| e0_7 | + x = 7 | 5.10 |
| e0_8 | + x = 8 (E8x) | -13.71 |
| e0_9 | + x = 9 | -27.90 |
| e0_A | + x = A | 12.64 |
| ea_mid | + result <= 64 | 5.35 |
| ea_hi | + result > 64 | 7.29 |
| e0_B | + x = B | 13.96 |
| eb_mid | + result >= 0 | 6.02 |
| eb_lo | + result < 0 | 7.94 |
| e0_C | + x = C | -9.27 |
| e0_D | + x = D (then note-delay check) | -5.75 |
| e0_E | + x = E | -40.86 |
| ee_set | + delay armed | 43.55 |
| e0_F | + x = F | 17.58 |

**Retrigger (E8x, EDx, E8x at tick 0)**

| item | meaning | cc |
|---|---|---|
| rt | to the location write | 30.32 |
| rt_len | location -> length write | 10.00 |
| rt_dma | length -> DMA-off write | 26.00 |

**Row advance, position change, end of tick**

| item | meaning | cc |
|---|---|---|
| adv | row advance base | 58.97 |
| adv_dn | + new pattern delay taken over | 53.00 |
| adv_dr | + delay repeats the row | -10.65 |
| adv_d0 | + delay ends | -20.75 |
| adv_l | + pattern loop jump | 32.01 |
| out | no position change: to the channel-0 volume write of the output phase | 241.26 |
| adv_np | row reached 64: to the position change | 186.36 |
| jc_np | position jump/break flag set: to the position change | 211.81 |
| np_end | position wraps (song end report), excluding the report itself | 9.05 |
| songend | song end report (host callback; approximate) | 75.48 |
| up_ls | unpack: per literal run | 29.50 |
| up_rs | unpack: per back reference | 36.00 |
| up_lb | unpack: per output byte (always 1024 bytes) | 6.00 |

