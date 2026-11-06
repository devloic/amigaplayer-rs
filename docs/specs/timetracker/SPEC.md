# TimeTracker (prefix `TMK.`): behaviour specification

*Sanitised for publication: behaviour only.*

TimeTracker is the replayer of the Eremation demo "We're Still Alive" (1993, BrainWasher & FireBlade),
packaged as the Eagleplayer "TimeTracker" (Wanted Team adaptation). Its pattern semantics are those of a
ProTracker 2.x style replayer, but it is **not** identical to ProTracker: it has its own packed module format,
fixed 50 Hz timing, several deviations in effect handling (section 6), a different register-write order,
and raster-line waits for the DMA restart. Everything in this document was checked against write logs of
the reference 68k player run through the host with DELI_WRITE_LOG (see LOGS.md); items marked
*(not loggable)* concern sample memory only.

Throughout: "word" is a big-endian 16-bit value; periods are Paula periods; "cc" is colour clocks
(3 546 895 Hz, PAL); a raster line is 227 cc.

## 1. Files and detection

### 1.1 Two-file form (what the Eagleplayer loads)
- Song file: name starts with `TMK.` (any case). Samples file: the same name with the prefix replaced by
  `SMP.`. A song file whose name ends in `.pat` (any case) instead takes the companion with that suffix
  replaced by `.SAM`.
- Detection: bytes 0..2 are `TMK` (0x54 0x4D 0x4B) and byte 3 is not 0. Nothing else is checked.
- Load check: the sum of all sample lengths (in bytes) must not exceed the samples file size, else the
  module is rejected as too short.

### 1.2 Single-file form (the form found in the public collections, `*.tmk`)
The same song data with the sample data appended directly after the last byte of row data of the song. In
all known files byte 3 is 1 and the sample data starts at `file length - total sample bytes`, which is
also exactly the end of the row data. An implementation should accept both forms; the replay is the same.
A stricter check that matches all known files: byte 3 = 1, song length 1..128, sample count 1..31, every
sample finetune <= 15 and volume <= 64, and the file large enough for header, positions, the row table of
the highest pattern and all samples.

### 1.3 Song file layout
| offset | size | content |
|---|---|---|
| 0 | 3 | `TMK` |
| 3 | 1 | number of subsongs N (1..255) |
| 4 | 2*(N-1) | for subsongs 1..N-1: byte size of that subsong's block (the last block's size is not stored) |
| 4+2*(N-1) | ... | N subsong blocks, back to back |

Subsong block layout (offsets from the block start):
| offset | size | content |
|---|---|---|
| 0 | 1 | song length L (number of positions) |
| 1 | 1 | bits 0..6: sample count C; bit 7: sample data is delta coded |
| 2 | 8*C | sample entries: length (words), repeat start (words), repeat length (words), finetune (byte, 0..15), volume (byte, 0..64) |
| 2+8C | L, padded to even | position list; each byte is pattern number * 2 |
| next | 128 per pattern | row table: for every pattern from 0 up to the highest one used, 64 words; each is the offset of that row's data, counted from the start of the row table |
| ... | ... | row data |

Only the **first** block's sample count, delta flag and sample entries are used for the samples; every
subsong plays with them. The sample entries of later blocks are skipped (their own count byte gives the
size to skip; their delta bit is ignored).

Row data: for each row, four voice entries follow one another (voice 0 first). Each entry is one of:
- `0x80`: empty (1 byte).
- `0xC0 + e` followed by a parameter byte: effect `e` (0..15) with that parameter, no note, no sample
  (2 bytes).
- `0x80 + n` (n = 1..63, bit 6 clear) followed by a sample byte `s`: note n with sample s (0 = none), no
  effect, parameter 0 (2 bytes).
- a byte `b` < 0x80 followed by two bytes `x`, `y`: note `b >> 1` (0 = none), sample
  `(b & 1) * 16 + (x >> 4)` (0 = none), effect `x & 15`, parameter `y` (3 bytes). This form also carries
  sample-without-note entries.

Note numbers 1..36 are C-1..B-3 (period table index note-1). Sample numbers count from 1.

Song info: subsongs N (the host numbers them from the first); song length = L of the chosen block;
sample count C; no title or sample names exist in the format.

### 1.4 Sample data
The samples are stored in entry order, each `length * 2` bytes, without gaps; sample k starts at the sum
of the byte lengths of samples 1..k-1. At load the player prepares the sample memory once:
- Delta coding (first block's bit 7): the whole sample area (all samples as one stream, total byte count
  = sum of lengths * 2) is decoded from its last byte towards its first: the last byte stays as stored,
  every earlier byte becomes (its stored value + the already-decoded byte that follows it) mod 256.
  (Note: a forward decoder that keeps the first byte and subtracts each following byte gives the same
  values shifted by one byte position; that is not what the reference plays.) *(not loggable)*
- Then, for every sample in entry order, the first four bytes at its start are set to zero (also when the
  sample is shorter than four bytes; the zeros then reach into the next sample). *(not loggable)*

## 2. Tables
- Period table: the standard ProTracker finetune table, 16 rows of 36 periods, row order finetune
  0, 1, ..., 7, -8, -7, ..., -1 (row index = the finetune byte 0..15). Row 0 is
  856 808 762 720 678 640 604 570 538 508 480 453 428 404 381 360 339 320 302 285 269 254 240 226
  214 202 190 180 170 160 151 143 135 127 120 113.
  The 16 rows lie back to back (576 periods); reads described below as running past the end of a row
  continue into the next row. Reads past the end of the whole table hit other player memory (unspecified;
  see README).
- Vibrato/tremolo sine (32 bytes): 0 24 49 74 97 120 141 161 180 197 212 224 235 244 250 253 255 253 250 244
  235 224 212 197 180 161 141 120 97 74 49 24.
- Funk table (16 bytes): 0 5 6 7 8 10 11 13 16 19 22 26 32 43 64 128.
- Volume: played volumes go through an identity table (0..64), i.e. unchanged.

## 3. State

Global: speed (initially 6), tick counter (0), song position (0), row (0), break row (0), position-jump
flag, pattern-loop flag, pattern-delay request and pattern-delay count (0), DMA-restart mask, the
portamento mask (initially **0**, see 6.3), LED state.

Per voice (all 0 at song start): note period (the finetune-0 period of the entry's note, or 0), effect,
parameter, sample start, length, original sample length, loop start, loop length, funk pointer, current
period, finetune, volume, tone-portamento target/direction/speed, vibrato command (speed/depth nibbles)
and position, tremolo command and position, wave control, glissando/funk byte (low nibble glissando,
high nibble funk speed), funk counter, sample-offset memory, pattern-loop row and count. The voice's DMA
bit is 1 << voice.

Song start (also on every subsong selection): the above reset; LED bit (CIA-A PRA bit 1) set = filter
**off**; playback enabled. The audio registers are not touched at song start.

## 4. Timing

The player has no tempo command. The host calls the player once per tick at 50 Hz: in the reference,
consecutive ticks start exactly 70 940 cc apart. Speed is in ticks per row. The tick counter is
incremented at the start of every tick; a row is read when it reaches the speed (it is then reset to 0),
so the first row of a song is played on the 6th tick.

## 5. Per-tick behaviour

Each tick:

1. **Volume refresh**: write AUDxVOL = voice volume for voices 0, 1, 2, 3 (the stored volume, not a
   tremolo value). (With a host master volume or balance below full, the value is scaled: voices 0 and 3
   by the left level, 1 and 2 by the right level, value * level / 64. At full volume this is a no-op.)
2. Increment the tick counter. If it is below the speed: run the **effect tick** (5.2) for voices 0..3,
   and the tick ends.
3. Otherwise reset the counter to 0. If the pattern-delay count is non-zero, run the effect tick (5.2)
   for voices 0..3 (with counter 0), then **advance** (5.4); no DMA restart this tick. Otherwise run the
   **row tick** (5.1) for voices 0..3, then advance (5.4), then the **DMA restart** (5.3).

### 5.1 Row tick, per voice (voices 0..3, consuming the row's entries in order)
The row's data is found through the row table: pattern = position byte / 2, row table entry
pattern * 64 + row.

1. If the voice's previous note period, effect and parameter are all zero (the previous entry was empty
   or a bare sample/note-less entry with no effect), write AUDxPER = current period. Otherwise **nothing
   is written** here: the period left by the last effect tick (for example a vibrato or arpeggio offset)
   stays in the register.
2. Decode the entry (1.3) into the voice: effect and parameter are replaced by the entry's (0 for the
   note+sample short form and for an empty entry). An empty entry also clears the note period; continue
   at step 6. An effect-only entry clears the note period; continue at step 6.
3. If a sample number is given: start = sample address, length = original length = sample length,
   finetune and volume from the sample entry. If its repeat start is non-zero: loop start = start +
   2 * repeat start, loop length = repeat length, and length = repeat start + repeat length; else loop
   start = start, loop length = repeat length. The funk pointer is set to the loop start. Write
   AUDxVOL = volume.
4. Note period = finetune-0 period of the note (0 if no note). If there is no note, continue at step 6.
5. Note present:
   - effect E with parameter 0x5x: finetune = x first;
   - effect 3 or 5: set up tone portamento (6.5) and continue at step 6 (no restart);
   - effect 9: run step 6 now (it is run again at the end, so the sample offset is applied twice to the
     stored start/length, once before the registers are written; see 6.9);
   - current period = period table[finetune][note-1];
   - effect E with parameter 0xDx: continue at step 6 (no restart now);
   - write DMACON = voice bit (DMA off); if wave-control bit 2 is clear, vibrato position = 0; if bit 6
     is clear, tremolo position = 0; write AUDxLCH/LCL = start, AUDxLEN = length, AUDxPER = current
     period; add the voice bit to the DMA-restart mask.
6. Row effects: first the funk step (6.13) if the funk speed is non-zero; then by effect:
   9 sample offset, B position jump, C set volume, D pattern break, E extended (6.12, with counter 0),
   F set speed. Other effects do nothing on the row tick.

### 5.2 Effect tick, per voice
1. Funk step (6.13) if the funk speed is non-zero.
2. If effect and parameter are both zero: write AUDxPER = current period; done.
3. By effect: 0 arpeggio, 1/2 portamento, 3 tone portamento, 4 vibrato, 5 tone portamento + volume
   slide, 6 vibrato + volume slide, 7 tremolo, A volume slide, E extended. Effects 8, 9, B, C, D, F
   write AUDxPER = current period and do nothing else.

### 5.3 DMA restart (only after a row tick)
1. Wait 9 raster lines (7.3), write DMACON = 0x8000 | DMA-restart mask (written even when the mask is
   0, as 0x8000).
2. Wait 9 raster lines, then for voices 0, 1, 2, 3: write AUDxLCH/LCL = loop start, AUDxLEN = loop
   length. This is done for every voice, also those not restarted and those never used (0 and 0).

### 5.4 Advance (after the row tick or a delay tick)
1. row = row + 1.
2. If a pattern-delay request is pending, the delay count takes it and the request is cleared.
3. If the delay count is non-zero, decrement it; if it is still non-zero, row = row - 1 (the row repeats).
4. If the pattern-loop flag is set: clear it, row = break row, break row = 0.
5. If row >= 64: next position (below). Then, while the position-jump flag is set: next position.

Next position: row = break row, break row = 0, clear the jump flag, position = position + 1; if it is
>= the song length: position = 0 and report **song end** to the host.

## 6. Effects

### 6.1 Arpeggio (0xy, effect tick only)
Counter mod 3 = 0: write the current period. 1: offset x, 2: offset y. For an offset, scan the voice's
finetune row from its first entry for 37 entries for the first period that is <= the current period, and
write the period `offset` entries further (both the scan and the offset may run into the next row). If
none is found, nothing is written.

### 6.2 Portamento up/down (1xx, 2xx, effect tick)
Amount = parameter AND the portamento mask; afterwards the mask is 0xFF. Up: period = period - amount;
if the low 12 bits of the period are below 113, the low 12 bits become 113 (the top 4 bits are kept).
Down: period + amount, the low 12 bits clamped to 856 the same way. Write AUDxPER = period AND 0xFFF.
(A slide below 0 wraps the 16-bit period and is then not clamped, as in ProTracker.)

### 6.3 The portamento mask (TimeTracker specific)
The mask is a single global byte. It is 0 after song start, so the **first** 1xx/2xx effect tick after
song start, on whichever voice, slides by 0 (the period is still written); from then on it is 0xFF.
E1x/E2x set it to 15 for their own slide and leave it at 0xFF.

### 6.4 Tone portamento (3xx, effect tick)
If the parameter is non-zero it becomes the slide speed and the parameter is cleared (the effect stays
3). If there is no target, nothing is written. Otherwise move the current period by the speed towards the
target (direction set at note time); on reaching or passing it, period = target and the target is
cleared. The written value is the period, or with glissando on (low nibble of the glissando/funk byte
non-zero): the first entry of the voice's finetune row (36 entries) that is <= the period, entry 35 if
none. Write AUDxPER.

### 6.5 Tone portamento setup (note with effect 3 or 5; TimeTracker specific target)
The target is looked up by scanning the **flat** period table starting at index 37 * finetune (not
36 * finetune: for finetune f the scan starts f entries into row f) for up to 37 entries, for the first
period <= the note's finetune-0 period; if none, the entry at offset 35 is used. For finetune 8..15, if
the found offset is not 0 it is reduced by 1. Target = that period. If target = current period: no
target. If target < current period: direction down (period decreases), else up. Effect: for finetune
f in 1..7 and note numbers up to f the target is a wrong (higher) note; for finetune 15 low
notes the scan leaves the table (unspecified).

### 6.6 Vibrato (4xy, effect tick)
Non-zero x replaces the vibrato speed, non-zero y the depth. Value: position p (byte); index
(p >> 2) & 31; waveform = wave control & 3: 0 sine table, 1 ramp (index * 8, or 255 - index * 8 when p
bit 7 is set), 2 and 3: 255. Amount = value * depth >> 7. Write AUDxPER = current period + amount, or -
amount when p bit 7 is set (the current period is not changed). Then p = p + speed * 4 (mod 256).
In TimeTracker the waveform is always sine and the position is always reset on a note (see E4x).

### 6.7 Tone portamento + volume slide (5xy), vibrato + volume slide (6xy) (TimeTracker specific)
The tone portamento (without changing its speed) or the vibrato (without changing speed/depth) runs as
above, including its period write; then the volume slide of 6.8 runs, which **writes AUDxPER = current
period again** before sliding. So with 6xy the register ends each tick at the unmodulated period: the
vibrato is not heard (only its position advances). With 5xy the glissando rounding is undone the same way.

### 6.8 Volume slide (Axy, effect tick)
Write AUDxPER = current period. If x is non-zero volume = min(64, volume + x), else volume =
max(0, volume - y). Write AUDxVOL = volume.

### 6.9 Sample offset (9xx, row tick)
A non-zero parameter is remembered. Offset = remembered value * 128 words. If offset >= length (signed
16-bit comparison) length = 1, else length = length - offset and start = start + 2 * offset. With a note
this is applied twice (5.1 step 5 and step 6): the restart uses the once-offset start and length, the
stored start/length (used by later notes without a sample number, retrigger, note delay and funk) are
offset twice.

### 6.10 Position jump, pattern break, volume, speed (row tick)
- Bxx: position = xx - 1 (mod 256), break row = 0, jump flag set, and **song end is reported at once**
  (every Bxx reports, also a jump forwards); the advance then moves to position xx (wrapping to 0 with a
  second report if xx >= length).
- Cxx: volume = min(xx, 64); write AUDxVOL.
- Dxy: row = 10 * x + y; if above 63 it is 0; break row = that row, jump flag set.
- Fxx: if non-zero, speed = xx and counter = 0. Every value is a speed (there is no BPM; F20..FF give
  very slow rows).

### 6.11 Tremolo (7xy, effect tick)
Write AUDxPER = current period first. Non-zero x/y replace tremolo speed/depth. Value as for vibrato with
the tremolo position q and waveform (wave control >> 4) & 3, except that the ramp waveform tests bit 7 of
the **vibrato** position. Amount = value * depth >> 6; vol = volume + amount (or - amount when q bit 7
is set), clamped to 0..64; write AUDxVOL = vol (stored volume unchanged). q = q + speed * 4.

### 6.12 Extended effects (Exy); "tick 0" means counter = 0 (row tick or delay tick)
Run on row ticks and on effect ticks (on effect ticks without a period write).
- E0x: LED: two writes to CIA-A PRA: first with bit 1 cleared, then with bit 1 = x & 1 (x = 0 filter on,
  x = 1 off). Every tick.
- E1x / E2x (tick 0): mask 15, then portamento up / down by x as 6.2; mask 0xFF afterwards.
- E3x: glissando = x. Every tick.
- E4x (TimeTracker specific): also sets **glissando** = x; the vibrato waveform cannot be changed.
- E5x: finetune = x (on a note row before the period lookup).
- E6x (tick 0): x = 0: loop row = current row. Else if the loop count is 0: count = x and loop; else
  count - 1 and loop unless it became 0. Loop = break row := loop row, pattern-loop flag set.
- E7x: tremolo waveform (high nibble of wave control) = x. Every tick.
- E8x: nothing.
- E9x (x > 0): retrigger (below) when counter mod x = 0, except at counter 0 when the entry has a note.
- EAx / EBx (tick 0): volume = min(64, volume + x) / max(0, volume - x); write AUDxVOL.
- ECx: when counter = x: volume = 0; write AUDxVOL = 0.
- EDx: when counter = x and the note period is non-zero: retrigger.
- EEx (tick 0): if the delay count is 0, pattern-delay request = x + 1 (the row is played x + 1 times).
- EFx (tick 0): funk speed = x; if non-zero, run a funk step at once.

Retrigger: write DMACON = voice bit (off), AUDxLCH/LCL = start, AUDxLEN = length; wait 9 raster lines;
DMACON = 0x8000 | voice bit; wait 9 raster lines; AUDxLCH/LCL = loop start, AUDxLEN = loop length,
AUDxPER = current period.

### 6.13 Funk step (TimeTracker: the old "funk repeat", not ProTracker's invert loop)
Funk counter = funk counter + funk table[funk speed]; when its bit 7 becomes set the counter is reset to
0 and the funk pointer moves on by loop length * 2 bytes; if it then lies above
start + 2 * original length - 2 * loop length (unsigned) it returns to the loop start. Write
AUDxLCH/LCL = funk pointer (Paula takes it at the next loop). No sample data is changed.

## 7. Register write order and timing

### 7.1 Order
Within a tick the writes come in the order of section 5: volume refresh (VOL0..VOL3), then per voice in
order 0..3 the writes of 5.1 or 5.2, then (row ticks) DMACON on and the 4 loop pairs. A 32-bit LC write
appears as LCH then LCL at the same clock. Values are exactly as above (verified write by write, 0
differing ticks over all logs, LOGS.md).

### 7.2 Gaps (cc, measured; see logs/gaps_summary.tsv for all observed pairs)
- Tick start to VOL0: 265 cc after the host's call. Volume refresh: VOL0 -> VOL1 181, VOL1 -> VOL2 208,
  VOL2 -> VOL3 219 (constant).
- Effect tick, voices without effect (period write only): VOL3 -> PER0 188, PER0 -> PER1 133,
  PER1 -> PER2 149, PER2 -> PER3 161; a second, 24 cc longer value of each of these (157 / 173 / 185)
  occurs depending on the preceding voice's path (see the table). The per-voice costs grow with the voice number for every
  volume/period write (the reference keeps a per-voice copy of each written value and finds the voice by
  successive comparisons): e.g. LEN -> PER in a note start 135 / 166 / 197 / 205 for voices 0..3.
- Note start (row tick): VOLx (sample volume) -> DMACON off 198..218 (voice 0), 222..225 (1), 240..249
  (2), 233..253 (3); DMACON -> LCH 57; LCL -> LEN 10; LEN -> PER as above.
- Loop block: LCH/LCL -> LEN 6, LEN -> next voice's LCH 18.
- LED pair (E0x): 10 cc apart.

### 7.3 Raster-line wait (DMA restart and retrigger)
The wait reads the beam's line counter and returns after it has changed 9 times. It polls about every
12.5 cc (alternately 13 and 12; the very first poll after the initial read comes 8 cc later, and after
each observed change the next round starts 17 cc after the detecting poll). The write after the wait
follows the 9th detecting poll by 54 cc (DMACON on) or 61 cc (first loop LCH/LCL). The second wait
starts 31 cc after the DMACON write. Observed result: the DMACON-on write always lies 53..65 cc after
a line boundary (h = 53..65), the first loop write 61..73 cc after the boundary 9 lines later
(DMACON on -> loop LCH 2042, 2054 or 2055 cc). The first wait starts after the last voice's row work and
the row advance: about 266 cc after a register write made by voice 3 as its last action (more when the
last voices wrote nothing; logs/gaps_summary.tsv, `row PERx/VOLx -> DMACON+on`, holds the
distributions).

## 8. Song end
Reported to the host on every Bxx and when the position wraps past the song length (5.4). The player
itself keeps playing; the reference host stops shortly afterwards.
