# SUN-Tronic ("SUNtronic") replayer: behaviour spec

*Sanitised for publication: behaviour only.*

Scope: the module format played by the UADE/Eagleplayer player "SUN-Tronic" (name shown by the host:
`SUNtronic`; configured file-name prefix `sun`). The player's own author line reads "Felix Schmidt, adapted by
Mr.Larmer/Wanted Team". A module of this format is not a data file: it is the sound driver of a game, a
relocatable 68k program image that carries its own tables, period table and song data. The player calls the
driver once per tick (50 Hz timer interrupt) and, for one of the two variants, patches the driver's end-of-song
points so that the driver reports the song end.

This is a behaviour description derived from write logs, DMA logs, execution-time histograms and a reading of
the 68k reference. It contains no code of the original. Cycle numbers are 68000 cycles as the reference machine
charges them (2 cycles = 1 colour clock, "cc").

**Is it the same format as "Tronic"?** No. The format `Tronic` (`TronicTracker(?)`, prefixes `dp`, `trc`, `tro`,
`tronic`, `tron`) is a pure data file (four sequence lists plus blocks, magic words 0x0058 0x00B0 in its
instrument table) interpreted by the player; SUN-Tronic modules are executable driver images with a different
detection rule, different data layout, different effects and different timing. The two share nothing but the
name stem. They need separate replayers.

Status (checked by a private model that follows only the behaviour described below, against the reference
runs; the model's per-step cycle costs are the numbers of Appendix T):

| item | result |
|---|---|
| variant A, real module `shades.sun` (3 074 ticks, full run to the song end) | value, order and tick-relative time of every register write equal the reference |
| variant A, DMA data (the words Paula fetches) for `shades.sun` (1 900 ticks) | equal |
| variant B, real module `msx-cracktro 2.sun` (4 999 ticks, 100 s, no song end exists) | value, order and time of every write equal; DMA data equal for 1 900 ticks |
| variant A, 6 synthetic modules (random waveform types 0..4, effects, arpeggios; 1 000 to 1 340 ticks each, to the song end) | registers equal in 5 of 6 runs; 2 runs (a004, a005) differ by one colour clock in the single write that follows the song-end notification of their first wrap tick, see 8.4; DMA data equal for the one run checked (a001, 1 189 ticks, all 5 waveform types) |
| variant B, 3 synthetic modules | registers and times equal up to the first read outside the module (undefined, 7.6): 1 272, 0 and 1 434 ticks |

"Tick-relative time" = colour clock of a write minus the clock at which the tick starts (8.1).

## 1. File format

### 1.1 What a module is

A module is one big-endian block: the driver code (first about 0x6A0 / 0x470 bytes, not described: the replayer
does what this text says instead of running it), then a data area whose layout is fixed by the driver version
(1.3, 1.4), then instrument data, wave data and the song's streams. Pointers inside the module are 32-bit
absolute addresses relative to the address the module was linked for; the player relocates them at load time
(1.5). The module needs no data from the player.

### 1.2 Detection

All of these hold (offsets from the start of the file; `n` = size):

- the 32-bit word at 0 is `0x48E7FFFE`, the 16-bit word at 4 is `0x4DFA`;
- the 32-bit word at 8 is `0x4A2E0018` (variant A, "tracker with end-of-song hook") or `0x4A2E0010`
  (variant B, "no song-end hook").

The original check does exactly this and nothing more. A third driver revision exists in the archive
(`pseudo oops.sun`: word at 8 is `0x4A2E07C0`); the player rejects it, so it is not part of this format.

The signed 16-bit word at offset 6 is the displacement of the data area: **D = 6 + displacement** is the offset of
the data area in the file (A: 0x6A4 in `shades.sun`; B: 0x46E in `msx-cracktro 2.sun`). All offsets in 1.3, 1.4
are relative to D.

For variant A the player also patches the driver's two end-of-list points (5.10); nothing of that is needed to
play, the replayer reports the song end itself.

### 1.3 Data area, variant A (word at 8 = 0x4A2E0018)

All offsets from D. "byte/word/long" are big-endian.

| offset | content |
|---|---|
| 0x00 | text `dos.library` (unused by the replay) |
| 0x14 | tick counter within a row (byte). Start value 5 |
| 0x15 | row counter (byte). Start value 0xFF |
| 0x16 | speed: ticks per row (byte). Start value 6; stream command 0x98 changes it |
| 0x17 | mask of the voices restarted in the current call (byte). Written by the replay |
| 0x18 | master volume (byte). In the module: 0x40. The end-of-list marker sets it to 1 |
| 0x19 | fade level (byte). Start value: the module's byte here (0 in the corpus) |
| 0x1A | fade enable (byte); also the value the fade counter is reloaded with. Start value 0 |
| 0x1B | second operand of command 0x93 (byte); not read by anything |
| 0x1C | fade counter (byte) |
| 0x1E | sequence-entry index (word). Start value 0 |
| 0x24 | noise word N1 (word), 0x26 noise word N2 (word). Start values: the two words found in the module here |
| 0x54 | period table: 72 words (0x54 .. 0xE3), ascending |
| 0xE4 | synthesised instruments: records of 0x24 bytes (1.6); index 0 is the first |
| 0x198 | (4 bytes; the first sequence entry overlaps the region where a sample-instrument table of 0x1C-byte records would be addressed; sample instruments (stream notes 0x01..0x3F) cannot exist in a valid module and are unspecified) |
| 0x19C | sequence list: entries of 0x14 bytes (1.7) |
| 0x4F3 | arpeggio table: sets of 16 bytes (1.8) |

The instrument records end where the sample-table region begins: the number of synthesised instruments is
(0x198 - 0xE4) / 0x24 = 5.

### 1.4 Data area, variant B (word at 8 = 0x4A2E0010)

| offset | content |
|---|---|
| 0x10 | tick counter (byte). Start value 4 |
| 0x11 | row counter (byte). Start value 0x0F |
| 0x12 | speed (byte). Start value 5; stream command 0x98 changes it |
| 0x14 | sequence-entry index (word). Start value 0x001F |
| 0x46 | period table: 72 words (0x46 .. 0xD5), ascending |
| 0xD6 | synthesised instruments: records of 0x22 bytes (1.6), 10 of them (0xD6 .. 0x229) |
| 0x22A | (4 bytes; unused) |
| 0x22E | sequence list: entries of 0x14 bytes (1.7) |
| 0x5A2 | arpeggio table: sets of 8 bytes (1.8) |

The start values above are what the player's initialisation leaves; they are those of a module whose first
sequence entry has a non-zero transposition byte (the case of the corpus module). The player has a second
initialisation, selected by a zero first transposition byte, that uses different start values and a voice record
of another size; it is unspecified here (no module uses it).

### 1.5 Pointers and relocation

Every pointer field is an absolute address. The file offset of the target is `pointer - O`, where the link
origin `O` is found as in the player: **O = (smallest pointer value found in the pointer fields of the
synthesised instruments) - a**, where `a` is the file offset of the first byte after the long word that ends
the sequence list (the first 32-bit word, at 4-byte steps from the start of the list, whose value is 0 or
0xFFFFFFFF). The pointer fields are the long words at record offsets 0x00, 0x08, 0x12, 0x1A, 0x1E (both variants). `O` is 0x50620 in `shades.sun`, 0x50914 in `msx-cracktro 2.sun`.

(The player relocates, in the instrument table, windows of 4 long words at 18-byte steps (A) or every long word
that looks like a pointer (B), then every long word up to the sequence list, then the four stream pointers of
each sequence entry. A replayer that works with file offsets does not relocate. One consequence matters:
reading outside the table of a record - the period table and arpeggio table accept indices that point beyond
them - reads relocated memory, so relocated pointers; see 7.6.)

### 1.6 Instrument record

Variant A (0x24 bytes):

| offset | size | content |
|---|---|---|
| 0x00 | long | pointer to the envelope table (bytes, one per tick) |
| 0x04 | word | envelope length (entries) |
| 0x06 | word | envelope loop position |
| 0x08 | long | pointer to the LFO table (bytes) |
| 0x0C | word | LFO table length |
| 0x0E | word | LFO loop position |
| 0x10 | word | LFO accumulator step |
| 0x12 | long | pointer to the morph table (bytes) |
| 0x16 | word | morph table length |
| 0x18 | word | morph loop position |
| 0x1A | long | pointer to wave A (bytes, signed 8-bit samples) |
| 0x1E | long | pointer to wave B |
| 0x22 | byte | HL: waveform length in words (the voice plays 2*HL bytes); 0x08..0x40 in the corpus |
| 0x23 | byte | waveform type 0..4 (5 and above behave alike, 5.9) |

Variant B (0x22 bytes): the same fields at the same offsets up to 0x1E (wave B pointer; a zero pointer selects
the feedback blend, 5.8); there is no HL and no type byte: the waveform length is always 16 words (32 bytes).

### 1.7 Sequence list

Entries of 0x14 bytes: four 32-bit stream pointers (voice 0 to 3), then four transposition bytes (voice 0 to 3).
The list ends with an entry whose first long word is 0 (variant A: the list wraps to entry 0; variant B: the
same). A first long word of 0xFFFFFFFF is the end-of-song marker of variant A (5.10). The player's relocation
and the song-end patch scan the list at 4-byte steps and stop at the first zero or 0xFFFFFFFF long word, so no
entry may contain such an aligned long word elsewhere (transposition bytes all zero would stop the scan).

Each entry plays 16 rows. `shades.sun` has 24 entries; `msx-cracktro 2.sun` has 32 entries. Variant B starts
with entry index 0x1F and adds one before its first use (5.2), so its list must have a terminator at index 32.

### 1.8 Arpeggio table

A: sets of 16 bytes, B: sets of 8 bytes. The set number is the voice's arpeggio number (stream command 0x9C);
the byte used is selected by the voice's arpeggio phase (0..15 in A, 0..7 in B), which counts up by one each tick
and wraps. The byte is subtracted from the note index of the period lookup (5.4). Values 0, 4, 7, 9, 12, 16 ... are
semitone offsets.

### 1.9 Streams

A stream is a byte sequence. One row is a run of events ended by a zero byte. Event bytes (5.3):

| byte | meaning |
|---|---|
| 0x00 | end of row |
| 0x40 .. 0x7F | start a note with synthesised instrument (byte - 0x40). (0x01..0x3F: sample instrument, unspecified) |
| 0x80 .. 0xB7 | commands (below); other values in this range are skipped, they consume no operand |
| 0xB8 .. 0xFF | pitch (not-byte) |

Commands (A has all, B only 0x9C, 0x9B, 0x9A, 0x99, 0x98; in B 0x9B has a one-byte operand):

| cmd | operands | effect |
|---|---|---|
| 0x9C | 1 byte | arpeggio number := operand; arpeggio phase := 0 |
| 0x9B | A: 2 bytes (high, low); B: 1 byte | pitch slide step (16-bit; B: the byte sign-extended) |
| 0x9A | 1 byte | level slide step (signed byte) |
| 0x99 | 1 byte | level := operand * 2 (byte); level slide step := 0 |
| 0x98 | 1 byte | speed := operand |
| 0x97 | 2 bytes (A) | noise word N1 := operand (16-bit, high byte first); N2 := operand xor 0x7E28 |
| 0x96 | none (A) | envelope position := 0 |
| 0x95 | none (A) | morph position := 0 |
| 0x94 | 1 byte (A) | pitch := (not operand) - transposition, as the high byte of the 16-bit pitch (low byte 0), pitch slide step := 0 |
| 0x93 | 2 bytes (A) | fade enable := first, fade reload byte := second |
| 0x92 | 1 byte (A) | fade level := operand |

Command 0x9B in A reads its two operand bytes, high byte first.

## 2. Structure of a tick

The host calls the driver once per tick. One call performs, in this order (details in 5):

1. **Stage 1**: write the audio registers for the voices (volume and period of all four voices; for voices that
   were started on the previous call also the DMA stop and the start address/length). Variant A does this on
   every call and starts with a DMACON write; variant B has two forms, 1a and 1b (5.6).
2. **Logic**: the tick counter is incremented; if it reached the speed, a new row begins (row counter, entry
   advance, stream parse of the four voices); the envelope/pitch step of the four voices follows.
3. **Stage 2** (A): the DMA restart and loop registers for the voices stopped in stage 1, then the copy of the
   synthesised waveforms into the buffers that Paula reads. (B does the restart inside stage 1a.)
4. **Waveform stage** (A): synthesise the next waveform of every synthesised voice into its second buffer.
   (B synthesises inside the envelope step.)

There is no other state change; each call is independent apart from the voice records and the globals.

## 3. Register writes of a tick (order)

Voice k = 0..3 owns Paula registers at 0xDFF0A0 + 0x10*k: LC (+0, high word; +2, low word; written as one 32-bit
write, both lines of the log have the same clock), LEN (+4), PER (+6), VOL (+8). DMACON is 0xDFF096.

Variant A, every tick (the reference log shows exactly these nine writes on a tick without a started voice):

1. DMACON := mask (clear bits of the voices that were started on the previous call; mask = 0 if none);
2. for k = 0..3: VOL, PER (in this order); if voice k is flagged as just started: LC, LEN after its PER;
3. DMACON := 0x8000 | mask;
4. for k = 0..3 whose loop length is non-zero (i.e. the started ones): LC, LEN.

Variant B:

- tick whose tick counter (before this call's increment) is non-zero: for k = 0..3: VOL, PER (8 writes);
- tick whose tick counter is zero (the call right after a new-row tick): DMACON := mask; for k = 0..3: VOL, PER,
  and LC, LEN if voice k is flagged; DMACON := 0x8000 | mask; then for flagged voices LC, LEN.

Values: VOL = (volume byte) * 0x101 (the byte duplicated into both halves); PER = the period word of 5.4;
LC = host address of the voice's waveform buffer (7.3) (+ 2 * loop start, which is 0); LEN = HL (A: instrument
byte at 0x22) or 16 (B) for the start write and for the loop write.

## 4. State

Globals: tick counter, row counter, speed, sequence-entry index, master volume (A), fade bytes (A), noise words
(A), restart mask (A). Per voice (voice records follow each other in host RAM; 7.3): stream pointer (32-bit);
instrument record pointer; pitch (16-bit, high byte = note index, low byte = fraction); pitch slide step
(16-bit); level (byte, 0..0x80) and level slide step (signed byte); arpeggio number, arpeggio phase; envelope
position (word); morph position (word); LFO accumulator (word) and LFO position (word); volume byte; period
word; mode byte (bit 7 = voice not started/inactive, bit 0 = synthesised voice, bit 1 = "buffer holds a valid
previous waveform"); start flag/length/loop words (word 0x1A/0x1C/0x1E in the voice record; A: the
start flag is the word at 0x1E, B: the word at 0x1A); remembered note byte (A); transposition byte; two waveform
buffers (A: 0x80 bytes each; B: 32 bytes each).

All voices start with the mode byte 0xFF, everything else 0, the stream pointers and transposition bytes of
sequence entry 0.

## 5. Replay

### 5.1 One call

Let `speed` be the global speed byte. A call does, in order:

1. stage 1 (5.6);
2. (A only) the fade step: if the fade enable byte is non-zero, decrement the fade counter byte; if the result is
   negative (bit 7 set) reload it from the fade-enable byte, then double the fade level byte; if the level is
   then >= 0x80 (negative) set level and enable to 0, otherwise if the level is >= 0x41 set the level to 0x40
   and the enable to 0. The level is not read by anything else: the step only changes the cost of the call;
3. tick counter := tick counter + 1 (byte);
4. if tick counter != speed: for each voice 0..3 the envelope/pitch step (5.4);
5. if tick counter == speed (new row): tick counter := 0; row counter := row counter + 1; if the row counter is
   0x10 then row counter := 0 and the sequence entry advances (5.10); then for each voice 0..3 in order: parse the
   voice's stream up to the end of the row (5.3), then run its envelope/pitch step;
6. stage 2 (A) and the waveform stage (A) (5.6, 5.7).

### 5.2 Start state and first call

At start the tick counter is 5, row counter 0xFF, speed 6 (A); 4, 0x0F, 5 (B). The first call is therefore a new
row (A: counter 6 == speed; B: the first call is an idle one, the second call is the first new row). In B the
row counter wraps to 0x10 on that call and the entry index advances from 0x1F to 0x20, which must be the list
terminator; the list then wraps to entry 0 (5.10) and the voices get entry 0.

### 5.3 Stream parse

The stream pointer points at the next event. The loop: take the next byte b; advance.

- b = 0: store the advanced pointer; the row ends.
- 0x01 <= b <= 0x7F: note start (below). b < 0x40 (sample instruments) is unspecified.
- 0x80 <= b <= 0xB7: a command (1.9). Commands apply the operand bytes as listed. Command bytes that are not in
  the table consume nothing.
- b >= 0xB8: pitch: pitch := ((not b) - transposition) & 0xFF in the high byte, low byte 0; pitch slide step := 0.
  (A) if the voice's remembered note byte is non-zero the note is started again with that byte (below); (B)
  nothing more.

Note start for a synthesised-instrument byte b (0x40..0x7F), instrument index i = b - 0x40:
instrument record := record i; remembered note byte := b (A); arpeggio number, arpeggio phase := 0; envelope
position := 0; morph position := 0; LFO accumulator := 0; LFO position := 0; mode := 1; wave start-flag words:
A: length word := HL, loop-start word := 0, loop-length word := HL, buffer address := the voice's first buffer;
B: start/length word := 16, loop-start := 0, loop-length := 16, buffer address := the voice's first buffer.
Pitch, level, level slide, slide step are not touched (level keeps the last value; the first value after
start is whatever commands 0x99/0x9A left). In A, the buffer address words and the lengths are the registers of
the voice's next start; the length word being non-zero is the start flag read by stage 1 on the next call.

### 5.4 Envelope step of a voice (called "E" in Appendix T)

If mode bit 7 is set nothing happens. Otherwise, with the instrument record `R`:

1. **Volume**. `e` = envelope byte at position (word `ep`). Volume byte := low byte of (((e * level) & 0xFFFF) >> 7)
   (A: then ((that * master) & 0xFFFF) >> 6, low byte). level is the voice's level byte (0..0x80). Write value
   later: VOL.
2. **Period**. `P` = pitch word. `q` = LFO byte at (LFO position); `x` = LFO accumulator; if x is negative
   (bit 15) x := -x (16-bit); `y` = x - 0x4000 (16-bit, signed); `z` = ((signed y * q) as 32-bit product >> 12,
   logical shift) low 16 bits; P' := (P + z) & 0xFFFF. Index i := ((P' >> 8) - arpeggio byte) & 0xFF, where the
   arpeggio byte is the byte at arpeggio set (arpeggio number) offset (arpeggio phase); the table word T[i] at
   period table + 2*i; f := P' & 0xFF. A: if f = 0 period := T[i], else period := (T[i] + (((T[i+1] - T[i]) & 0xFFFF)
   * f >> 8)) & 0xFFFF. B: if f = 0 period := T[i], else period := ((T[i] * (0x100 - f) + T[i+1] * f) >> 8)
   & 0xFFFF. The period is stored for the next stage 1.
3. (B, if mode bit 0) the waveform blend (5.8).
4. **Pitch slide**: pitch := (pitch + slide step) & 0xFFFF. A only: if the result has bit 15 set, add 0x4800;
   then if (result - 0x4800) is non-negative (as a 16-bit signed subtraction), subtract 0x4800.
5. **Level slide**: s := level + sign-extended level slide step (16-bit); if s < 0 level := 0; else if s >= 0x81
   level := 0x80; else level := s (low byte).
6. Arpeggio phase := (phase + 1) & 0xF (A) / & 7 (B).
7. Envelope position := position + 1; if it equals the envelope length (R + 4) it becomes the envelope loop (R + 6).
8. LFO accumulator := (accumulator + LFO step (R + 0x10)) & 0xFFFF; LFO position := position + 1; if it equals the LFO length
   (R + 0xC) it becomes the LFO loop (R + 0xE).
9. (B, if mode bit 0) morph position := position + 1; if it equals the morph length (R + 0x16) it becomes the
   morph loop (R + 0x18).

Equality tests are exact: a loop point at or beyond the table end, or a start position beyond the length, runs
into 7.6.

### 5.5 What the synthesised voice plays

Each voice with mode bit 0 plays a looping waveform buffer of 2*HL bytes (A) or 32 bytes (B) whose content is
rebuilt every tick (5.7, 5.8). The buffer content that Paula fetches is the content after the call (A: after
the copy at the end of stage 2; B: after the copy of stage 1a/1b of the call that follows), see 7.2.

### 5.6 Stage 1, stage 2 (A and B)

**A stage 1** (each call): m := bit k set if voice k's start-flag word (voice record word 0x1E) is non-zero; the
restart mask; write DMACON := m (clear). Per voice k: write VOL, write PER; if the start flag is non-zero write LC
= buffer address (the buffer is the voice's first buffer), LEN := start-length word (voice word 0x1A); copy the
loop description: loop address := buffer address, loop-offset and loop-length words := voice words 0x1C, 0x1E.
Remember m (saved for stage 2).

**A stage 2**: write DMACON := 0x8000 | m; wait (970 cycles, 8.2); per voice with non-zero loop length (the copy of
word 0x1E made in stage 1, which is the start flag): write LC := loop address + 2 * loop offset, LEN := loop
length; clear the voice's start-flag word 0x1E. Then, for each voice with mode byte non-zero, copy the voice's
second buffer to the first buffer: HL >> 1 long words (4 bytes each), at least 1.

**B stage 1a** (call when the tick counter byte at call start is 0): as A's stage 1 with the same writes (the start
flag is the word 0x1A), but (a) the start LC/LEN writes use the start length word, (b) afterwards the second
buffer of every voice (not just the flagged ones) is copied to its first buffer (8 long words), (c) a wait of
2 570 cycles, DMACON := 0x8000 | m, then the loop writes for the flagged voices (LC := buffer address + 2*loop
offset, LEN := loop length word 0x1E) and the start flags are cleared.

**B stage 1b** (all other calls): per voice VOL, PER; if mode bit 0 is set copy the second buffer to the first
(8 long words).

### 5.7 Waveform stage, variant A

For each voice 0..3 with mode bit 0 set and bit 7 clear: `m` := morph byte at the voice's morph position; `cnt` :=
2 * HL bytes; wave A = the bytes at the instrument's wave-A pointer, wave B at its wave-B pointer; the result
goes into the voice's second buffer (A: voice record offset 0xA6, 0x80 bytes). Types by the instrument's type
byte:

- **0 (cross-fade)**: out[i] := (A[i] + (((signed B[i] - signed A[i]) * m) & 0xFFFF as signed, >> 7 arithmetic)) & 0xFF.
- **1**: if m = 0xFE: out := copy of wave A (mode bit 1 unchanged). Otherwise: source S := voice's second buffer
  itself if mode bit 1 is already set, else wave A; set mode bit 1. If m = 0xFF (noise): w := N1; for each of the HL output words: w := (((w * w) >> 8) & 0xFFFF) xor 0xAC91, stored as the next big-endian output word; after the last word N2 := ((((N2 * N2) >> 8) & 0xFFFF) xor 0x6E47) and N1 := (last w) xor N2 (16-bit values). Otherwise (smoothing): k :=
  (0x80 - m) & 0xFF; previous := signed S[cnt-1]; for i in 0..cnt-1: previous := (previous + ((((signed S[i] -
  previous) * k) & 0xFFFF) as signed >> 7)) & 0xFFFF; out[i] := low byte of previous.
- **2 (splice)**: first m bytes := wave B bytes 0..m-1, rest := wave A bytes from m (m <= cnt); m = 0 is the
  copy of wave A.
- **3 (resample)**: d := ((signed byte m) + 0x40) & 0xFFFF; step1 := (0x8000 / d) << 8 (integer quotient of the
  low 16 bits, 24-bit fixed point); n1 := ((d * (cnt-1)) & 0xFFFF) >> 7 (the number of the first-part samples minus
  one), n2 := (cnt - 1) - n1. Position accumulator pos := 0; for n1+1 outputs: out := A[(pos >> 16) as signed 16-bit
  index]; pos += step1. If n2 - 1 >= 0: d2 := 0x80 - d; step2 := (0x8000 / d2) << 8; n2 outputs more with the same rule using step2 (accumulator continues).
- **4 (low-pass)**: source buffer := the voice's second buffer if mode bit 1 is set, else wave A; set mode bit 1
  (the filter state is the last byte of the source buffer). c := (m + 0x20) & 0xFFFF; r := 0x000FFFE0 / c (if the
  quotient does not fit 16 bits r := 0xFFE0); r := (r - 0x26 * signed m) & 0xFFFF. x := (signed last byte of
  the source buffer) << 7 (16-bit). For i in 0..cnt-1: x := (x + 2 * ((((signed A[i] << 7) - x) as signed 16 * signed r) >> 16 (low
  16 bits))) & 0xFFFF; out[i] := (x >> 7) & 0xFF (logical).
- **5 and above**: if mode bit 1 is clear: set it and copy wave A into the second buffer; else nothing changes.

After each synthesised voice: morph position := position + 1; if it equals the morph length (R + 0x16) it becomes
the morph loop (R + 0x18).

### 5.8 Waveform blend, variant B (inside the envelope step, mode bit 0, after the period store)

`m` := morph byte at the morph position (0..0xFF); k0 := (0x80 - m) as a signed 16-bit value; wave A bytes are signed.

- wave B pointer non-zero: for i = 0..31: out[i] := (((signed A[i] * k0) + (signed B[i] * m)) & 0xFFFF as signed
  >> 7 arithmetic) & 0xFF.
- wave B pointer zero (feedback blend): source := the voice's second buffer if mode bit 1 is set, else wave A; set
  mode bit 1; previous := signed source[31]; for i = 0..31: previous := (((signed source[i] * k0) + (previous * m)) &
  0xFFFF as signed) >> 7; out[i] := low byte of previous. (When the source is the second buffer, the buffer is
  overwritten in place; source[i] is read before out[i] is written.)

### 5.9 Types 5 and above (A)

See 5.7: first time the voice plays wave A as is, afterwards the second buffer keeps its previous content.

### 5.10 Sequence advance, wrap and end of song

When the row counter reaches 0x10 the entry index is incremented (word) and the entry looked up. Let the first
long word of the entry be v:

- v = 0xFFFFFFFF (A only): master volume := 1 and the host is notified of the song end (7.5); the entry is then
  treated as ordinary (its four "pointers" are the marker itself and following bytes: undefined, 7.6). No module of
  the corpus contains the marker.
- v = 0 (end of list): entry index := 0, (A) the host is notified of the song end once more (it costs 482 cycles,
  8.4), and the entry lookup is repeated with entry 0.
- otherwise the four pointers of the entry go into the voices' stream pointers and the four transposition bytes
  into the voices' transposition byte.

Variant B has no song-end notification: a B module plays forever, the host's run is stopped by its own time limit.

## 6. Initial writes before the first tick

None. The first register write of a run is the first write of the first call (A: DMACON := 0 at call start + 219 cc;
B: the VOL of voice 0 at +189 cc). The host's init calls before it (about 147 and 45 600 cycles of non-register
host work) write no Paula register.

## 7. Details and edge cases

### 7.1 Arithmetic

All stored fields are the width given; "signed" means sign-extension of the stated width; products named
"& 0xFFFF" are truncated to the low 16 bits before the shift (the multiplier of the original produces a 32-bit
result and the following 16-bit shift takes the low word); "(a * b) >> n" for the LFO scaling is the 32-bit
product, logical shift, low 16 bits.

### 7.2 What Paula fetches

A voice's buffer is read by DMA continuously. The reference DMA logs show the buffer words of the first buffer as
they are after the copy at the end of the call (A) or after the copy of stage 1a/1b of the next call (B); a
replayer that renders audio from the writes needs the buffer content at the time of each DMA fetch: both copies
are done at the cycle positions given in Appendix T (the copy steps), the buffer is rewritten word by word at about
34 cycles per long word.

### 7.3 Host addresses written to LC

Voices keep their data in a host-allocated block (one record per voice, records contiguous); LC is the address of
the first buffer inside the record. In every reference run the block starts at 0x7FF588: record k is at
0x7FF588 + k * 0x130 (A) / + k * 0x68 (B); the first buffer is at +0x26 (A) / +0x22 (B) in the record. So LC for voice 0
is 0x7FF5AE (A) / 0x7FF5AA (B), the values seen in the logs. A replayer may choose any chip-RAM address; the
lead decides if log equality of LC is wanted.

### 7.4 Volume and period values

VOL is written as the volume byte duplicated (0x1F1F ...), not limited: values above 0x40 occur only if the
envelope data is. PER is the full 16-bit period (observed 0x00 .. 0x0FFF).

### 7.5 Song-end notification

A only. When a wrap or the marker occurs the driver reports the song end to the host once per occurrence. The
reference host stops the run a few ticks later (the logs end 2 to 3 ticks after the wrap). Detecting the song
end for a replayer: the first wrap of the sequence list (entry terminator reached). The module has no
position, length or title information; the title is the file name.

### 7.6 Undefined behaviour

Out-of-range reads go to the neighbouring bytes of the module (relocated memory, 1.5), then beyond the module
into host memory: a pitch index above 71 (arpeggio byte too large, a note above the table), a morph value of a
type that indexes beyond the wave (type 2 with m > 2*HL, type 3 with a zero divisor: the original traps), a stream
that runs into the next data, a list without terminator, a loop point beyond the table length, a first sequence
entry with a long word of 0xFFFFFFFF. The synthetic runs diverge after the first such read and are reported as
"up to the first undefined read".

## 8. Timing

### 8.1 Tick start

The host raises one interrupt per 70 940 cc (50 Hz of the CIA). A call starts at a colour clock S_k; the first
call's S_0 and its phase belong to the host (reference runs: 98 786 / 98 816 for variant A runs, 111 533 for
variant B). Write time = S_k + 76 + floor(T / 2), where 76 is the interrupt entry in cc and T the cycles counted
from the start of the driver call up to and including the instruction that writes the register (the step that
contains it ends with it, see Appendix T). The spacing of S_k is 70 940 cc, with small deviations of the host
(the log's call starts alternate by a few cc; the write times relative to the call start, which is what the
steps give, are exact).

### 8.2 Costs

All steps of Appendix T are cycle counts of the reference machine (conditional branches are charged their mean,
multiplications and divisions a constant). Delay loops: A stage 2: 97 passes of 10 cycles (the three set-up
cycles are in T3); B stage 1a: 257 passes of 10 cycles. Cycle counts are exact integers; conversion to cc is the
floor of half the cycle count at the point of each write.

### 8.3 Typical times

Variant A, idle tick (nine writes), cc after call start: DMACON 219, then VOL/PER per voice at about 245/255,
320/330, 396/406, 471/481, DMACON (set) about 2 200 - 5 500 (depends on the envelope steps run before it). Variant
B idle tick: first write VOL(0) at about 189 .. 200.

### 8.4 Song end and rounding

In variant A the wrap of the entry list inserts a 482-cycle block (the notification call and the lost
re-synchronisation) at the point after the zero test of the entry (K14 in Appendix T) and the lookup repeats
(K13, K14 again). The one-colour-clock deviation noted in the status table appears in 2 of the 7 wrap ticks
measured: the DMACON write that follows (stage 2) is 1 cc earlier than T/2 predicts; the cause (a half-cycle
carry in the host's accounting of the notification call) was not determined. All other writes of those ticks are
exact.

## Appendix T: step costs

Step costs in 68000 cycles. A step's cost is charged when its behaviour occurs (condition in the text); steps
that end with a register write end with that write. Ids are my own labels.

### Variant A

Per-command costs of the stream parse (P10 is charged once for every command-test step passed, including the matching
one; tests are tried in the order 0x9C, 0x9B, 0x9A, 0x99, 0x98, 0x97, 0x96, 0x95, 0x94, 0x93, 0x92; then the body):

0x9C: 42, 0x9B: 64, 0x9A: 26, 0x99: 50, 0x98: 26, 0x97: 80, 0x96: 26, 0x95: 26, 0x94: 78, 0x93: 42, 0x92: 26

| id | cycles | step |
|---|---|---|
| E1 | 23 | envelope step entry (a voice whose mode byte has bit 7 set stops here after E2) |
| E2 | 16 | inactive voice: return |
| E3 | 244 | volume: envelope byte, level, master |
| E4 | 145 | set-up of the period computation (arpeggio index, pitch, LFO fetch) |
| E5 | 4 | extra when the LFO accumulator is negative |
| E6 | 106 | LFO scaling and addition to the pitch |
| E7 | 75 | table index and fraction test |
| E8 | 100 | extra when the fraction is non-zero: interpolation |
| E9 | 49 | store period, add the pitch slide |
| E10 | 20 | extra when the slid pitch is negative |
| E11 | 25 | upper-limit test of the slid pitch |
| E12 | 20 | extra when the slid pitch reached the limit |
| E13 | 49 | level slide: add, sign test |
| E14 | 14 | level negative: set to zero |
| E15 | 17 | level non-negative: compare with 0x81 |
| E16 | 8 | extra when level >= 0x81: clamp to 0x80 |
| E17 | 97 | store level, arpeggio phase + 1, envelope position + 1 and compare |
| E18 | 20 | extra when the envelope position reached its length: reload loop position |
| E19 | 77 | LFO accumulator += step, LFO position + 1 and compare |
| E20 | 20 | extra when the LFO position reached its length: reload |
| E21 | 16 | return |
| P8 | 31 | note start entry (store note byte; test bit 6) |
| P9 | 304 | synthesised-instrument start: initialise the voice |
| P1 | 12 | stream parse entry |
| P2 | 19 | fetch next stream byte (every iteration) |
| P3 | 28 | byte 0: store the stream pointer, return |
| P4 | 11 | test sign of the byte (note byte 0x01..0x7F vs 0x80 and above) |
| P5 | 19 | byte >= 0x80: compare with 0xB8 |
| P10 | 17 | per command-test step (1 .. 11, until the command matches) |
| P11 | 10 | byte 0x80..0xB7 that is not a known command: skipped |
| P6 | 83 | byte >= 0xB8: set the pitch (up to the test of the remembered note) |
| P7 | 10 | remembered note non-zero: continue with the note start (P8, P9) |
| S1 | 20 | stage 1 entry |
| S2 | 21 | per voice: test the start flag (4 times) |
| S3 | 12 | extra per voice whose start flag is set (4 x at most) |
| S4 | 16 | up to and including the write of DMACON (clear, mask of started voices) |
| S5 | 32 | set-up of the voice loop |
| S6 | 20 | per voice: up to and including the write of AUDxVOL |
| S7 | 20 | per voice: up to and including the write of AUDxPER |
| S8 | 21 | per voice: test the start flag |
| S9 | 24 | voice started: up to and including the write of AUDxLC (both halves, same instant) |
| S10 | 20 | voice started: up to and including the write of AUDxLEN |
| S11 | 90 | per voice: end (copies the loop description, loop control) |
| S12 | 28 | stage 1 end |
| T1 | 24 | stage 2 entry |
| T2 | 16 | up to and including the write of DMACON (set | mask) |
| T3 | 4 | set-up of the delay loop; followed by 97 passes of 10 cycles (970) |
| T4 | 32 | after the delay loop: set-up of the voice loop |
| T5 | 21 | per voice: test the loop length |
| T6 | 48 | loop length non-zero: compute the loop address |
| T7 | 12 | loop: write of AUDxLC (both halves) |
| T8 | 20 | loop: write of AUDxLEN |
| T9 | 16 | loop: clear the start flag |
| T10 | 34 | per voice: end |
| T11 | 20 | after the voice loop: set-up of the buffer copy |
| T12 | 21 | per voice: test the voice mode byte (non-zero = synthesised voice) |
| T13 | 53 | synthesised voice: compute the number of long words to copy |
| T14 | 8 | set-up of the copy |
| T15 | 34 | per long word copied (n+1 of them) |
| T16 | 22 | per voice: end of copy step |
| T17 | 16 | stage 2 end |
| Y1 | 20 | waveform stage entry |
| Y2 | 27 | per voice: test mode bit 0 |
| Y3 | 23 | test mode bit 7 |
| Y4 | 147 | compute morph value, loop length, pointers |
| Y5 | 128 | type 0: per output byte |
| Y6 | 10 | type 0: end |
| Y7 | 25 | type != 0: dispatch |
| Y8 | 17 | type 1: test the morph value 0xFE |
| Y9 | 22 | copy loop: per byte (types 1 with 0xFE, 5 and above) |
| Y10 | 10 | copy loop end |
| Y11 | 25 | type 1 (morph not 0xFE): test the buffer-valid flag (mode bit 1) |
| Y12 | 4 | extra when mode bit 1 is already set: source is the previous output |
| Y13 | 20 | set mode bit 1 |
| Y14 | 17 | test the morph value 0xFF |
| Y15 | 20 | type 1, morph 0xFF (noise): set-up |
| Y16 | 104 | noise: per 16-bit word |
| Y17 | 136 | noise: update of the two noise words |
| Y18 | 30 | type 1, other morph: set-up |
| Y19 | 112 | type 1 smoothing: per output byte |
| Y20 | 10 | type 1 smoothing: end |
| Y21 | 25 | type >= 2 dispatch (test type 2) |
| Y22 | 25 | type 2: set-up |
| Y23 | 22 | type 2: per byte copied from wave B |
| Y24 | 15 | type 2: between the two copies (morph > 0 path) |
| Y25 | 22 | type 2: per byte copied from wave A |
| Y26 | 10 | type 2: end |
| Y27 | 25 | type >= 3 dispatch (test type 3) |
| Y28 | 184 | type 3: first-segment set-up (division, counts) |
| Y29 | 90 | type 3: first segment set-up end |
| Y30 | 44 | type 3: first segment, per byte |
| Y31 | 15 | type 3: test for a second segment |
| Y32 | 184 | type 3: second segment set-up (division) |
| Y33 | 44 | type 3: second segment, per byte |
| Y34 | 10 | type 3: end |
| Y35 | 25 | type >= 4 dispatch (test type 4) |
| Y36 | 29 | type 4: set-up, test the buffer-valid flag |
| Y37 | 4 | type 4: extra when mode bit 1 is already set |
| Y38 | 312 | type 4: set-up of the filter coefficient (division, products) |
| Y39 | 80 | type 4: initial state |
| Y40 | 210 | type 4: per output byte |
| Y41 | 10 | type 4: end |
| Y42 | 25 | type >= 5: test the buffer-valid flag |
| Y43 | 30 | type >= 5, flag clear: set it (copy follows: Y9, Y10) |
| Y44 | 16 | waveform stage end |
| Y45 | 49 | per voice: morph position + 1 (synthesised voices) or the same path with the position left alone |
| Y46 | 20 | extra when the morph position reached its length: reload loop position |
| Y47 | 22 | per voice end |
| K1 | 166 | call entry, set-up, up to the call of stage 1 (stage 1 follows immediately) |
| K2 | 21 | after stage 1: test whether a fade is active |
| K3 | 25 | fade active: decrement the fade counter |
| K4 | 57 | fade counter went negative: reload it, double the fade level |
| K5 | 32 | extra when the doubled level is negative: level and fade flag cleared |
| K6 | 25 | compare fade level with 0x41 |
| K7 | 32 | extra when level >= 0x41: level = 0x40, fade flag cleared |
| K8 | 51 | tick counter + 1, compare with speed |
| K9 | 20 | counter != speed (idle tick): voice loop set-up |
| K10 | 18 / 22 | idle tick: per voice 18 before the envelope step and 22 after it (voice 3: the 22 is the loop exit, same value) |
| K11 | 57 | counter == speed (new row): clear counter, row counter + 1, compare with 16 |
| K12 | 40 | row counter == 16: clear it, entry index + 1 |
| K13 | 109 | entry lookup (per pass): address of entry, test for the 0xFFFFFFFF marker |
| K14 | 27 | entry test for zero (end of list) and branch |
| K15 | 220 | entry accepted: copy its 4 stream pointers and 4 transpositions into the voices |
| K16 | 20 | new-row tick: voice loop set-up |
| K17 | 18 | new-row tick: per voice 18 before the stream parse |
| K18 | 18 / 22 | new-row tick: per voice 18 before and 22 after the envelope step (as K10) |
| K19 | 10 | new-row tick: end of voice loop |
| K20 | 18 | call of stage 2 (18); stage 2 follows |
| K21 | 18 | call of the waveform stage (18); it follows |
| K22 | 148 | end of call (restore, return) |

### Variant B

Command bodies (cycles; test step P10 charged as in A, tests tried in the order 0x9C, 0x9B, 0x9A, 0x99, 0x98): 0x9C: 42, 0x9B: 38, 0x9A: 26, 0x99: 54, 0x98: 16

| id | cycles | step |
|---|---|---|
| E1 | 23 | envelope step entry |
| E2 | 16 | inactive voice (mode bit 7): return |
| E3 | 160 | volume |
| E4 | 145 | period computation set-up and LFO |
| E5 | 4 | extra when the LFO accumulator is negative |
| E6 | 106 | LFO scaling and addition |
| E7 | 79 | table index |
| E8 | 166 | extra when the fraction is non-zero: interpolation |
| E9 | 12 | store period |
| E10 | 25 | test mode bit 0 |
| E11 | 117 | synthesised voice: morph value, blend set-up |
| E12 | 16 | blend with wave B: set-up |
| E13 | 174 | blend with wave B: per output byte (32) |
| E14 | 10 | blend end |
| E15 | 27 | feedback blend: set-up, test mode bit 1 |
| E16 | 4 | extra when mode bit 1 is already set |
| E17 | 36 | feedback blend: set mode bit 1, initial state |
| E18 | 162 | feedback blend: per output byte (32) |
| E19 | 36 | add the pitch slide |
| E20 | 41 | level slide: add, sign test |
| E21 | 14 | level negative: set to zero |
| E22 | 17 | level non-negative: compare with 0x81 |
| E23 | 8 | extra when level >= 0x81: clamp |
| E24 | 97 | store level, arpeggio phase + 1, envelope position + 1 and compare |
| E25 | 20 | extra at envelope end: reload loop position |
| E26 | 77 | LFO accumulator += step, LFO position + 1 and compare |
| E27 | 20 | extra at LFO end: reload |
| E28 | 25 | test mode bit 0 |
| E29 | 49 | synthesised voice: morph position + 1 and compare |
| E30 | 20 | extra at morph end: reload loop position |
| E31 | 16 | return |
| P1 | 12 | stream parse entry |
| P2 | 19 | fetch next stream byte |
| P3 | 28 | byte 0: store pointer, return |
| P4 | 9 | test sign |
| P5 | 19 | note byte: test bit 6 |
| P6 | 274 | synthesised-instrument start |
| P7 | 270 | byte 0x01..0x3F (not used by the corpus: sample instrument start; unspecified) |
| P8 | 19 | byte >= 0x80: compare with 0xB8 |
| P9 | 70 | byte >= 0xB8: set pitch |
| P10 | 17 | per command-test step (until the command matches) |
| P11 | 10 | byte 0x80..0xB7 not a known command: skipped |
| S1 | 20 | stage 1a entry |
| S2 | 21 | per voice: test the start flag |
| S3 | 12 | extra per voice whose start flag is set |
| S4 | 16 | up to and including the write of DMACON (clear, mask) |
| S5 | 32 | set-up |
| S6 | 20 | per voice: up to and including AUDxVOL |
| S7 | 20 | per voice: up to and including AUDxPER |
| S8 | 21 | per voice: test the start flag |
| S9 | 24 | voice started: up to and including AUDxLC |
| S10 | 20 | voice started: up to and including AUDxLEN |
| S11 | 26 | per voice: end |
| S12 | 28 | set-up of the buffer copy |
| S13 | 118 | copy of the 4 voice buffers, per voice (8 long words each) |
| S14 | 8 | set-up of the delay |
| S15 | 8 | after the delay (delay = 257 passes of 10 cycles = 2570) |
| S16 | 16 | up to and including the write of DMACON (set | mask) |
| S17 | 32 | set-up of the voice loop |
| S18 | 21 | per voice: test the start flag |
| S19 | 52 | voice started: compute the loop address |
| S20 | 12 | loop: AUDxLC write |
| S21 | 20 | loop: AUDxLEN write |
| S22 | 16 | loop: clear the start flag |
| S23 | 26 | per voice: end |
| S24 | 16 | stage 1a end |
| S25 | 32 | stage 1b entry |
| S26 | 20 | per voice: up to and including AUDxVOL |
| S27 | 20 | per voice: up to and including AUDxPER |
| S28 | 25 | per voice: test mode bit 0 |
| S29 | 12 | synthesised voice: set-up of the copy |
| S30 | 34 | synthesised voice: per long word copied (8) |
| S31 | 26 | per voice: end |
| S32 | 16 | stage 1b end |
| K1 | 157 | call entry, set-up |
| K2 | 10 / 18 | tick counter == 0 (previous tick was a new-row tick): 10 before stage 1a, 18 after it |
| K3 | 18 | tick counter != 0: call of stage 1b (18) |
| K4 | 51 | tick counter + 1, compare with speed |
| K5 | 20 | counter != speed (idle tick): voice loop set-up |
| K6 | 18 | idle tick: per voice 18 before, 18 after the envelope step |
| K7 | 57 | counter == speed (new row): clear counter, row counter + 1, compare with 16 |
| K8 | 40 | row counter == 16: clear it, entry index + 1 |
| K9 | 103 | entry lookup (per pass) and zero test |
| K10 | 26 | entry is zero: index := 0 (loop back to K9) |
| K11 | 220 | entry accepted: copy its 4 stream pointers and 4 transpositions |
| K12 | 20 | after the entry step (row counter == 16 path) |
| K12 | 20 | after the entry step (row counter != 16 path) |
| K13 | 18 | new-row tick: per voice 18 before the stream parse |
| K14 | 18 | new-row tick: per voice 18 before and 18 after the envelope step |
| K15 | 10 | new-row tick: end of voice loop |
| K16 | 148 | end of call |
