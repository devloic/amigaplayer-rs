# PowerTracker (`*.pt`): behaviour spec

Scope: the PowerTracker format (Laxity / Kefrens): modland's `Powertracker/Merge/`, the music of two
Kefrens intros, "graphics are none" and "inspiration is gone". A different format from the one of
the Laxity port (`src/replay/laxity.rs`, `powt.*` / `pt.*`, Eagleplayer `Laxity-Player`), which
claims neither module.

Method: black box only. The reference is DeliTracker's DeliPlayer `PowerTracker` (PlayerStore, it
names itself "PowerTracker v1.20") on this project's 68000 engine, without a genie. Observed: the
Paula register writes with their colour clocks (`DELI_WRITE_LOG`), the audio words Paula fetched
(`DELI_DMA_LOG`), the host calls it made, the guest memory it plays its waveforms from (dumped after
each of its calls, in a private engine copy that is not part of the repository), and how all of that
changes for 615 built modules (the layout of section 2 with edited headers, records, lists and
rows: differential probing). The player was not disassembled or read, and no source of it was used
(an Aminet source named in a readme was not fetched). No code or data of the player is in this text;
the tables are the values seen played.

Times are colour clocks ("cc", 3 546 895 a second), or CPU cycles where said (two a colour clock).

## 1. Reference

| item | finding |
|---|---|
| DeliPlayer `PowerTracker` | plays both modules and every probe of section 2's layout; 50 ticks a second (one call a frame); calls `dtg_SongEnd` when the position list wraps (section 9); two `dtg_WaitAudioDMA` calls a tick (section 8) |
| genie | none: the player writes Paula itself (no NotePlayer) |
| subsongs | one |

## 2. Module layout and detection

All words big-endian.

| offset | size | meaning |
|---|---|---|
| 0 | word | file offset of the tracks (T) |
| 2 | word | bytes of the instrument records (R, 24 per record) |
| 4 | word | song length L: positions per list |
| 6 | word | restart position |
| 8 | R bytes | instrument records 1 .. R/24 (section 3) |
| 8 + R | word | `01 00` in both modules |
| 10 + R | 4 × L bytes | the position lists of channel 0, 1, 2, 3, one byte a position (section 4) |
| T | 256 × n bytes | tracks: 64 rows of 4 bytes each |
| then | | sample data (section 3) |

In both modules T = 10 + R + 4L. Detection (the port's): R ≠ 0, R a multiple of 24, 1 ≤ L ≤ 255,
restart < L, T = 10 + R + 4L, the word at 8 + R is `01 00`, the file reaches T.

The reference also plays modules whose word at 8 + R is not `01 00` (probes `pre0`, `seq0000`,
`seq0001`, `seq0101`, `seq0200`, `seqb0101`, `seqb0200`, `seqb0300`, `chk_pre_ffff`), with a layout
not worked out; the port rejects them (section 11). Modules whose header breaks the equalities above
were rejected by both (28 probes).

## 3. Instruments and samples

A record is 24 bytes: 16 bytes of name, then four words w0, w1, w2, w3. Byte 6 of the tail (w3's
high byte) is the detune, signed (section 5); byte 7 (w3's low byte) the kind: 0 a sample, any other
a pulse wave (section 7). Instrument numbers are 1 to 15; a record is read where it would lie even if
past R (the module's other bytes).

Sample instrument: w0 the length in words, w1 the loop start in words, w2 the loop length in words.
The samples follow the tracks: the first starts at T + 256 · ((m + 1) & FF), m the largest byte of
all four lists (`FF` in a list gives a start of T, as the byte count wraps); sample i starts 2 · Σ w0
of the records 1 .. i−1 after it. Its loop starts 2 · w1 bytes into the sample, w2 words long; w2 = 0
loops a silent word (one word of 0 the player holds). A sample with w0 = 0 starts nothing.

Pulse instrument: w0's low byte the step delay, w1 the minimum width, w2 the maximum width
(section 7). It plays the voice's 32-byte buffer (length 16 words), looped.

## 4. Lists and rows

Each tick of a row (section 8) reads, per channel, the byte of its list at the current position:

| list byte | meaning |
|---|---|
| `00`-`7F` | track number t: the row at T + 256 t + 4 · row |
| `80`-`FD` | the same, t taken signed (reads before the tracks; the port reads 0 before the file) |
| `FE` | the channel is left alone: no row read, no write |
| `FF` | no row read; volume 0 written; the voice's pulse width turns to narrowing (section 7) |

A row: byte 0 note, byte 1 volume byte, byte 2 instrument (high nibble) and effect (low nibble),
byte 3 parameter.

## 5. Notes, periods, detune

Notes 1 to 60 are 5 octaves of the top octave
`214 202 190 180 170 160 151 143 135 127 120 113`, shifted left by 4 − octave (note 1 = 3424,
note 60 = 113). Any other note index gives period 0 (the reference reads past its table there).

Note byte:

| note | meaning |
|---|---|
| 0 | no note |
| 1-3F | the note (with effect 3: the slide target, section 6.3) |
| 40-7F | note off: instrument 0, the voice's loop the silent word, pulse narrowing; the low 6 bits trigger the note n & 3F with the voice's (new) instrument nibble, but the loop stays silent |
| 80-FF | note off as above, the instrument nibble is ignored; with n & 3F ≠ 0 the voice's DMA bit is set (section 8) but nothing else is written |

A triggered note with an instrument (sample with w0 ≠ 0, or pulse) writes, in this order, the
location, the length and the period (base period = note period + detune · D[note]), and the
voice's loop becomes the instrument's. The detune step D per note 1 .. 60 (measured):

```
24 24 20 20 20 18 16 16 16 14 14 12 12 12 10 10 10  9  8  8  8  7  7  6  6  6  5  5  5  4
 4  4  4  3  3  3  3  3  2  2  2  2  2  2  2  1  1  1  1  1  1  1  1  1  1  1  1  1  1  1
```

The offset is computed at the trigger from the triggered note and kept for the arpeggio's notes.
A trigger also sets the voice's DMA bit in the tick's mask (section 8), with or without an
instrument. An instrument nibble without a note still sets the voice's instrument, detune, loop and
pulse state.

## 6. Volume byte and effects

### 6.1 Volume byte

| byte | at the row | every tick of the row, after the row's writes |
|---|---|---|
| 00-7F | the voice's volume | — |
| 80-8F | volume kept | volume += low nibble, at most 40 (written) |
| 90-9F | volume kept | volume −= low nibble, at least 0 (written) |
| FE | volume kept | a break, as effect D (section 6.3), on every tick of the row |
| A0-FD, FF | volume kept | nothing |

The row writes the volume of every channel read, after the instrument and before the trigger's
writes. Two channels with FE make one break.

### 6.2 Row effects

| fx | at the row | on the other ticks |
|---|---|---|
| 0 | — | arpeggio when the parameter ≠ 0 (6.4) |
| 1 xx | — | period −= xx (written) |
| 2 xx | — | period += xx (written) |
| 3 xx | the note is the target | slide, 6.3 |
| 4 xy | speed x, depth y if non-zero | vibrato, 6.4 |
| 5 xy | counter 0 | every x ticks volume + 1 (x ≠ 0), else every y ticks volume − 1; 0 .. 40 |
| 6 | — | nothing written (it only takes time) |
| 7, 8, 9 | — | period written again |
| A xy | — | period written again, then volume + x (x ≠ 0) or − y, 0 .. 40 |
| B xx | position jump (6.3) | period written again |
| C xx | volume xx written (the voice's volume is not changed) | period written again |
| D | break: next position, row 0 | period written again |
| E | two byte writes of 0 to CIA-A port A (`BFE001`) | period written again |
| F xx | speed xx (ticks a row) | period written again |

Speed starts at 6.

### 6.3 Jumps, breaks, slides

B xx: position = ((position & FF00) | ((xx − 1) & FF)) + 1, row 0: so B 00 goes 256 positions on
from position 0 (`B00full_*` probes). B wins over D in the same row. A break (D, or FE) goes to the
next position, row 0. Only reaching position L wraps: to the restart position, and SongEnd
(section 9); a jump past L reads list bytes past the lists (the tracks' bytes).

Effect 3 (tone portamento) slides only down: at the row the target is the period of note & 3F; when
it is below the voice's period the period falls by xx on every tick of the row after the first, past
the target, to the row's end; when it is above or equal nothing is written.

### 6.4 Vibrato, arpeggio

Vibrato, every tick but the row's: delta = (S[pos & 7] · depth) >> 7 with
S = `0 24 49 74 97 120 141 161`, period = base − delta when pos & 20 else base + delta, then
pos += speed (pos a byte, 0 at each trigger). Written every tick, also with parameter 0.

Arpeggio (effect 0, parameter xy ≠ 0): on tick c of the row (counting from 0 at the row),
c mod 3 = 0: note, 1: note + y, 2: note + x; period of that note + the trigger's detune offset,
written every tick. Notes past 60 give period 0 plus the offset in the port; the reference reads past
its table there (section 11).

## 7. Pulse waves

Each voice has a 32-byte buffer. A wave of width w (0 .. 32) is: bytes 0 .. 15 − ⌊w/2⌋ hold 80,
then w bytes of 00, then 7F to the end. Before any note the buffers hold widths 1, 1, 13, 32.

A pulse instrument (row with its nibble) sets: width = w1, rising, counter 0, delay = w0's low byte,
bounds w1 .. w2. Every tick, for each voice whose pulse is on, after the effects: counter − 1 (a
byte); while the result is ≥ 0 (signed) it is kept and nothing happens; otherwise the counter is
reloaded with the delay and the width steps: rising, +1, and at or past the maximum it is set to the
maximum and turns falling; falling, −1, and at or below the minimum it is set to the minimum and turns
rising. So delays 0 .. 7F step every delay + 1 ticks, 80 every 129, 81 .. FF every tick. Then the
buffer is rewritten with the width (clamped to 0 .. 32 for the shape). Widths above 33 (`big_24`,
`big3_24`) and 33 itself (the reference writes one byte past the buffer) are section 11.

Turns to falling without other change: a list byte FF, a note off (40-FF), a sample instrument
nibble (the wave goes on in the buffer, unheard).

## 8. Tick and write order

1. Row tick (speed counter 0): for channel 0 .. 3: the list byte (section 4); the volume write; on a
   trigger location, length, period; the volume byte's tick check (6.1); the row effect (C writes
   the volume, E the CIA); then the position / row advance (6.3).
   Effect tick: for channel 0 .. 3: the volume byte's check, then the effect (6.2); a FE break
   advances after the four channels.
2. Speed counter + 1, back to 0 at the speed.
3. The pulse waves, voice 0 .. 3 (section 7).
4. If any voice triggered: DMACON with the mask (off).
5. On a wrap: SongEnd.
6. `dtg_WaitAudioDMA`; DMACON on with the mask last written in step 4 (set bit + its voices; 0 until a
   note started one), every tick; `dtg_WaitAudioDMA`; then for voice 0 .. 3 location and length of the voice's
   loop (section 3, 7; the silent word, length 1, for a voice without one).

The engine's `dtg_WaitAudioDMA` waits to the start of the tenth raster line from now.

## 9. Song end

The reference calls `dtg_SongEnd` when the position reaches L (by the row advance, a break or FE),
and plays on from the restart position. The port reports the song end at that tick. A B jump that lands
on L wraps as well; one past L does not.

## 10. Times

The write times were reproduced by adding up CPU cycles per step of the work (the table `K` in
`src/replay/powertracker.rs`: some 120 weights for the tick's start, every branch of a row, of the
volume check, of each effect, of the pulse step (per byte of the width written) and of the tick's
end), fitted by least squares on 4 108 rows of event times from 541 probes and the two modules. 17
rows remain unmet, all by at most 12.5 cc (B / D jumps with lists shorter than 4 positions, `big_21`).

The song (the first tick, the audio's origin) starts after the player's init, which takes
497 + 54 · L + 29 · (R / 24) + 6 · (new maxima while scanning the four lists for their largest byte)
colour clocks, as counted by the port's builder.

## 11. Unobserved / port choices

- Header word 8 + R other than `01 00`: the reference plays it, the port rejects the module.
- Arpeggio notes above 60, notes 40 .. 7F with an instrument past its silent loop in odd orders
  (`r_41`, `notes64`): the reference reads past its period table; the port gives period 0 (+ detune).
- Negative track numbers (list bytes 80 .. FD) reach guest memory before the module in the reference;
  the port reads zeros before the file.
- Pulse bounds above 33, instrument numbers whose record lies outside the module, instrument 0 with
  pulse state left over (`w_noins`): the reference's buffers go off in ways not reproduced.
- Position counter: kept to 16 bits (a guess; never reached).

## 12. Check of the port against the reference

`src/replay/powertracker.rs` follows this text. Writes compared with the reference's to its song end:

| module | writes | value, order, colour clock | buffers (every tick) | WAV 48 kHz |
|---|---|---|---|---|
| graphics are none (92.1 s) | 53 759 | all equal | 4 604 of 4 604 ticks | bit-identical, 4 420 800 frames |
| inspiration is gone (72.9 s) | 42 614 | all equal | 3 644 of 3 644 ticks | bit-identical, 3 499 200 frames |

Same song end in both. Probes (615 built modules, 3 s each): 539 of 617 (with the two modules) equal
in every write value, order and colour clock; 540 with bit-identical WAVs; 28 rejected by both; 9
played by the reference but rejected by the port (section 2). Of the 38 that differ: 24 arpeggios
above note 60, 2 pulse widths above 33, 4 negative tracks, 5 cases with the reference reading
garbage (`chk_trk_0e`, `trkbase_after`, `w_noins`, `notes64`, `r_41`), 3 with timing only (≈ 12 cc:
`B_00_1_0_1`, `fxb_23`, `r_19_30_2_b_00`). The port takes none of 8 652 files of the other formats'
corpus lists.
