# Jochen Hippel ST (hst.*, sdc.*) - behaviour specification

*Sanitised for publication: behaviour only.*

Slug `jochenhippelst`. Player name in the reference: "Jochen Hippel ST". This is a different engine from
the repository's `hippel.rs` (COSO / TFMX / 7V, Amiga-native synthesiser). The module container is shared
with it (the same COSO image, the same pitch-program and instrument command bytes in part), but this
driver was written for the Atari ST: it keeps a software model of a YM2149 sound chip (three tone
channels, one noise generator, a mixer, per-channel volume) and, once per tick, converts the YM state into
Paula writes. Do not reuse the Amiga voice engine; reuse at most the COSO image parser idea.

All numbers are decimal unless prefixed `$` (hexadecimal). "cc" = colour clocks (3.546895 MHz PAL
units, 227 per raster line).

## 1. Overview

* One tick per video frame of the host (the reference interrupt entry is exactly 70940 cc apart at the
  start, see section 9).
* A tick = `Play` (advance the three voices and, on a counter, the sequencer, producing a YM register
  image) followed by `the Paula conversion step` (turn the YM image into Paula writes).
* Paula channels: YM channel A -> Paula channel 0 ($DFF0A0 block), YM channel B -> Paula channel 3
  ($DFF0D0 block), YM channel C -> Paula channel 2 ($DFF0C0 block). The digital/"SID" voice -> Paula
  channel 1 ($DFF0B0 block). (So the register blocks used are $A0, $D0, $C0, $B0 for A, B, C, S.)
* Tone is a 4-byte pulse sample looped with a period derived from the YM period; noise is a 1024-byte
  noise buffer looped, with the period from a table; silence is a 1-word silent sample with volume 0.
  The digital voice plays a 1-byte "level" sample (volume-as-sample, the Atari "SID" trick) or a real
  sample from a table.
* The module itself stays untouched; the module memory is read-only for the replayer (except one
  subsong-table patch, section 5).

## 2. File format and detection

### 2.1 Containers

A module file is one of:

1. **COSO image** - starts with `COSO` and has a header of 7 big-endian longs at +4..+$1C (offsets from
   the image start), followed by the data; the bytes at +$20 are `TFMX` or `MMME`.
2. **Raw TFMX-style module** - starts with the tag `TFMX` (or `MMME`), header words, then fixed-size
   blocks. The player converts it to a COSO image in RAM (section 3).
3. Either container may be **preceded by prefix code** (68k code of the original Atari/Amiga conversion
   driver, as found in many "hst." files). The player locates the tag from a short pointer-forming
   instruction in the first 128 words of the file; the offset of the tag from the file start is called
   `Size` below. The module proper then starts at `Size`.

Two module families occur (`hst.` and `sdc.` prefixes): both are handled by the same detection; the
prefix only decides the file name of the companion sample file (2.4).

### 2.2 Detection (all conditions are on the module bytes)

Work at the module start (or at the offset found via the prefix, 2.1):

* If the 4 bytes at offset 0 are `MMME`, or the 4 bytes at +$20 are `MMME` after `COSO`: accept.
* If the tag is `TFMX` at offset 0 (or reached through the prefix pointer): require that the word at +4
  is below $200 and the word at +16 is not zero. Then look at the first byte of every pitch program
  block (the fixed 64-byte blocks beginning right after the header): count blocks whose first byte is
  $E2 and whose next byte is positive (call this P) versus those whose next byte is negative (call this
  N). Accept when N >= P.
* If it starts with `COSO`: require the word at +$30 to be non-zero, the long at +$18 to be non-zero and
  `TFMX` or `MMME` at +$20.
* Otherwise reject.

### 2.3 COSO image header

All offsets are relative to the image start. Longs unless stated.

| offset | meaning |
|---|---|
| +4 | pitch-program ("frequency sequence", FS) table |
| +8 | instrument ("volume sequence" owner) table |
| +12 | pattern table |
| +$10 | position list: 12 bytes per position, 4 per voice (voice 0,1,2): pattern number, transpose A, transpose B, flag byte |
| +$14 | subsong table: 6 bytes per subsong: start position (word), end position (word), speed (word) |
| +$18 | extra table (6 bytes per entry), count in the word at +$32 (count-1) |
| +$1C | digital-sample table base (see 2.5) |
| +$24 (word) | number of pitch programs minus 1 |
| +$26 (word) | number of instruments minus 1 |
| +$30 (word) | number of subsongs |
| +$32 (word) | extra-table count minus 1 |
| +64 (word) | "short tables" flag |

If the word at +64 is non-zero the table entries (pitch programs, instruments, patterns) are **16-bit signed
offsets** from the image start ("short"); otherwise they are 32-bit offsets. A position entry's pattern
number indexes the pattern table, an instrument number indexes the instrument table, a pitch-program
number indexes the pitch-program table.

For a raw module with prefix code the "short" flag is additionally cleared when no indirect-jump
instruction word $4EFB can be found while scanning the prefix (scan the prefix word by word; if the tag offset is reached without seeing it, the type is the old one). So
for raw modules the prefix chooses between two **command sets** (section 6):

* **new type** (a jump table is present in the prefix), 16-bit offsets;
* **old type**, 32-bit offsets.

For a COSO image the command set follows the same flag at +64 (non-zero = new type).

### 2.4 Companion sample file

If the 4 bytes at +$1C of a raw module are `LSMP`, the digital sample table is in a second file named
like the module file with the leading `HST.` (case-insensitive) replaced by `SMP.`; if that does not
exist, the file `SMP.set` in the same directory is tried. The companion's first bytes are the sample
table of 2.5 (base = companion start). The companion is loaded to a host-chosen memory address; when
the digital voice plays such a sample, the Paula start-address writes point into that address, which the
implementer may choose freely (the reference's value is host-dependent). One synthetic module of this
kind is in the logs folder (hst.lsmp1 + SMP.lsmp1); the value/order of writes is as for the in-module case.

### 2.5 Digital sample table

Entries are 8 bytes: word 0 = start offset (relative to the table base), word 2 = end offset or marker,
words 4 and 6 are unused here. The table starts with a word of $80 or $100 (the first entry's start,
i.e. the table size) and ends at the first entry whose first word is zero. Derive:

* A table exists when the word at the computed table position is $80 or $100. For a COSO image the table
  position is the long at +$18 plus 6 bytes times the extra-table count; if the long at +$18 is zero or
  the word there is not $80/$100, there are no digital samples (and the base at +$1C is only used as a
  base for the sample pointers). For a raw module the position is computed from the header (section 3).
* `d3` = the first word of the last entry with a non-zero first word: that is the end of the sample area.
  "has samples" = last first-word non-zero.
* Entry rules, used by the digital command (section 6.3): entry n starts at start(n) = word 0 of entry n.
  If word 2 is 0: the sample length is start(n+1) - start(n) bytes, played once, then the loop is the
  1-word silent sample. If word 2 is $10: the sample is 8 words long and loops itself. Otherwise word 2
  is the end offset; the length is end - start bytes, then a silent loop.

### 2.6 Subsongs, title, length

* Number of subsongs = word at +$30. Subsong n (1-based) uses the subsong-table entry n-1 (6 bytes at
  the subsong table + 6*(n-1): start word, end word, speed word). The host's initial subsong is 1.
* No title or author text exists. Return an empty title.
* There is no stored length. The song ends when voice 0 has consumed (end - start + 1) positions
  (section 8). A reasonable implementation reports song end through the host's end-of-song callback at
  that moment and loops from the start (the reference host stops at the callback).

## 3. Raw-module loading (conversion to a COSO image)

Header words at the tag (all big-endian words; offsets from the tag): w4 = number of pitch programs - 1,
w6 = number of instruments - 1, w8 = number of patterns - 1, w10 = number of positions - 1, w12 = pattern
size in bytes, w16 = number of subsongs - 1, w18 = extra-entry count - 1. The data follows the $20-byte
header at +$20 in this order:

1. (w4+1) pitch-program blocks of 64 bytes each;
2. (w6+1) instrument blocks of 64 bytes each;
3. (w8+1) patterns of w12 bytes each;
4. (w10+1) positions of 12 bytes;
5. (w16+1) subsong entries of 6 bytes;
6. (w18+1) extra entries of 6 bytes;
7. digital sample table (when present).

The digital table's offset from the tag is: ((2 + w4 + w6) * 64) + (1 + w8) * w12 + (1 + w10) * 12 +
(2 + w16 + w18) * 6 + 32. (The "2 +" and "+32" come from the 32-byte header and the off-by-one counts;
verified on real and synthetic modules.)

The player builds a COSO image from this data and then works only on the image:

* The image begins with `COSO` followed by 7 long offsets (filled in as the tables are built) and a copy
  of the 32 raw header bytes at +$20.
* Pitch-program blocks and instrument blocks are copied into the image each trimmed: scan the 64-byte
  block from its end backwards for the first byte that is $E1 (keep through that byte) or $E0 (keep through
  its argument byte); if neither is found keep the whole block. Each table entry is a 16-bit offset to the
  trimmed block.
* Patterns are re-encoded into a compact "row stream" (section 7.1): rows of 2 or 3 bytes, with `$FE n` /
  `$FD n` timing markers and `$FF` as end of pattern. Each raw pattern slot is w12 bytes = w12/2 rows of 2
  bytes: byte 0 = note (0 = empty row, 1 = end of pattern), byte 1 = instrument/flag byte; if
  (byte1 & $E0) != 0 the row has an extra third byte, which in the raw pattern is the byte *before* the
  note byte (the previous row's last byte, or the byte immediately before the pattern for the first row).
  Empty rows are merged into the previous row's wait (the compact form emits `$FE d` where d is the
  number of empty rows before the next real row, minus nothing; the first real row at row index r > 0
  emits `$FD (r-1)` once before it). The compact stream ends with `$FF`.
  Behaviourally what matters: each real row is delivered with a *delay* (in ticks counted by the row
  speed) equal to 1 + number of empty rows that follow it; the first row of a pattern is delayed by the
  number of empty rows that precede it. The reference's numbers are reproduced by the private model; the
  implementer may instead interpret the raw patterns directly with the same rule as long as the
  observable tick counts per row are identical (rows consumed one per `speed` ticks, empty rows
  consuming one slot each).
* Positions, subsongs and extra entries are copied; the digital table position follows.

Because the conversion only matters through the observable row timing and values, an implementer can
interpret the raw form directly. The pattern end is at the first row whose note byte is 1, or after w12/2
rows.

## 4. Host interface

The player runs from the host's per-frame callback. Registers at call are zero except the host
globals. Song end is signalled by the host's end-of-song function; the reference host stops playing at
that point. `--subsong N` in the reference means "step the subsong counter N times" (relative), which is
only visible as extra silence writes at start; the player-visible init is the same.

## 5. Subsong initialisation

For subsong n:

1. Read start, end, speed from the subsong table. A speed of 0 becomes 4.
2. If the signed 16-bit difference end - start is negative, the table entry is replaced by 0,0 (start =
   end = 0) and the module memory is patched accordingly (observable only on a replay after the same
   subsong; keep the patch).
3. Set the tracker counter to 1 (so the sequencer runs on the first tick), the tracker speed to the
   table speed and `songpos count` = end - start.
4. For each of the 3 voices: clear all voice variables; "null" instrument = the 8-byte stream
   `01 00 00 00 00 00 00 E1` used until the first row is loaded; "null" pitch program likewise. Set
   voice number 0..2, the position pointer to start*12 + voice*4 + position-list base, the position
   advance offset to 12, the row-wait counters to 0, the volume-program wait counters (b22/b23) to 1,
   the SID-mode byte (voice 0 only) to $FF (none), and load the first position entry: pattern number,
   transpose A (voice variable "tA"), transpose B ("tB"), flag byte. If the high nibble of the flag byte
   is $F, the low nibble is the voice's volume reduction (section 7.3).
5. YM shadow registers: tone/noise mixer shadow = $3B, voice C's volume byte = $10 (but see the emulation reset
   below), voice A/B volumes 0, noise shadow 0; digital state = idle. Paula state variables start
   "no channel running".

`the emulation reset` (also invoked at song end, section 8) resets the YM shadows to the values above and clears
the digital voice state.

## 6. Voice processing (per voice, per tick)

Per tick, in this order: voice 0 update, then the SID section (section 6.6), voice 1 update, voice 2
update, YM mixer registers stored, tracker counter step (section 7).

A voice update runs (a) the pitch program part, (b) the volume program part, (c) the period / mixer /
vibrato / portamento calculation, and stores the YM image words for the voice.

Voice state (all bytes unless noted): note byte, row byte, row flags byte, instrument number-derived
fields (vibrato speed, vibrato delay, vibrato depth), volume (from the volume program), volume reduction,
transpose A/B, vibrato state, portamento accumulator (32-bit), pitch-program pointer + index, volume
program pointer + index, program wait counters, mixer mode (0 tone, 1 noise, 2 off), noise number.

### 6.1 Pitch program ("FS") part

If the pitch-program wait counter is non-zero: decrement it and do nothing else this tick.

Otherwise read bytes from the pitch program at the current index:

* A byte that is not a command is a **note**: store it as the voice's note byte, advance the index past
  it, stop.
* Commands ($E0..$EF; note that in the old type bytes >= $EB are notes, and if the module has no
  digital samples $EA is a note too; bytes > $EF or < $E0 are always notes):

| cmd | arg bytes | effect |
|---|---|---|
| $E0 | 1 | jump: new index = arg (old type: arg & $3F); continue |
| $E1 | 0 | hold: index set back to point at this command (so it stays on it); new type also copies the previous byte into the note byte; stop |
| $E2 | 0 | restart the volume program (volume index 0, volume wait counter 1); continue |
| $E3 | 2 | set vibrato speed = first arg, vibrato depth = second arg (these are the instrument's fields); continue |
| $E4 | 1 | mixer mode = tone; noise number = arg; continue (the arg is stored as the noise value for this tick only) |
| $E5 | 0 | mixer mode = noise (tone off); continue |
| $E6 | 0 | mixer mode = off (silent) ; continue |
| $E7 | 1 (new type) / 0 (old) | old type: clear the digital/SID voice (state idle); new type: the program counter is reloaded to the very start of the module image (this reproduces the reference result for any argument value because the host's low memory is zero; treat as "restart the pitch program from the image start", see README); continue |
| $E8 | 1 | wait: pitch wait counter = arg; if it is non-zero decrement it and stop for this tick; otherwise continue |
| $E9 | 1 | skip one byte; continue |
| $EA | 1 (new) | new type: row flags byte = $20 (enable portamento), portamento speed byte = arg; continue. Old type with digital samples: same as $EC |
| $EB | 1 | set the "pitch-end" byte (voice's high-bit mask for the SID source; value arg); continue |
| $EC | 1 or 2 | play digital sample (6.3) |
| $ED | 0 | digital state = idle (stop the digital voice); continue |
| $EE | 1 | SID source byte (voice 0 only meaningful; arg) ; continue |
| $EF | 1 | second SID parameter byte (arg); continue |

(In the old command set the pitch-program number inside rows is reduced with `& $3F` in jumps; in the
new set the arg is a full byte.)

Notes: The note byte has bit 7 set meaning "absolute note", otherwise it is relative: after reading,
effective note = (note byte + row note byte + transpose A) & $FF when bit 7 is clear; then & $7F.

### 6.2 Volume program ("VS") part

If the volume-hold counter is non-zero, decrement it and stop. Otherwise decrement the volume wait
counter; if it is non-zero, stop. Reload the wait counter from the instrument's speed byte (copy kept).
Read bytes from the volume program at its index:

* A byte below $E0 is the new volume (0..$3F in normal use); store it and advance, stop.
* $E0 n: jump to index (n - 5), continue (old type n & $3F).
* $E1: stop at this command (hold the current volume; new type also reloads it from the byte before).
* $E8 n: set hold counter = n; if non-zero decrement and stop, else continue.
* Other command values do not occur in valid data (the reference's behaviour is undefined; treat as stop).

The "volume program" of an instrument starts at instrument byte 5 (instrument stream +5).
Instrument stream layout: byte 0 = volume speed (initial wait counter), byte 1 = pitch program number,
byte 2 = vibrato speed, byte 3 = vibrato depth, byte 4 = vibrato delay, then the volume program.

### 6.3 Digital sample command ($EC, and $EA when the file has digital samples and is old type)

Argument 1 is the sample number n. Look up entry n of the digital table (2.5):

* word 2 = 0: playback length is start(n+1) - start(n); loop is the silent 1-word sample.
* word 2 = $10: length 8 words, loops itself.
* otherwise length = end - start, loop silent.

The sample start address is the table base + start (table base = module address of the table, see 2.5).
Sample volume is 64. The Paula period for this voice is the module "Period" constant (section 9.4),
unless the file is of the nibble-converted kind (2.5 / 9.5): then a second argument byte gives a note: if
its bit 7 is clear the note is (arg + row note + transpose A) & $FF; the low 7 bits index a 96-entry table
of $400 * 2^(i/12) (rounded down) values; if the index is below 96 the period is
((Period * $2000) / (that value / 8)) + 1 (integer division; if the quotient overflows 16 bits keep the
low 16 bits of the product), else Period is kept. Then set the digital state to "start" (state 1).

### 6.4 Period, mixer, noise

After the two programs, the effective note n = ((note byte + row byte + tA) & $FF if bit 7 clear) & $7F.
Base period p0 = YM period table[n] (section 10.1). The mixer mode (set by $E4/$E5/$E6) edits the
shadow mixer register (bits 0..2 tone-off per channel, bits 3..5 noise-off): mode 0 (tone) clears both
the tone and noise bits for this channel (both enabled); mode 1 (noise) sets the tone bit and clears the
noise bit, and sets the noise value = (row byte + note byte) & $FF if note bit 7 clear, else note byte &
$7F; mode 2 (off) clears the tone bit and sets the noise bit. If a noise value is non-zero this tick
(from $E4's arg or mode 1), the shadow noise-period register becomes the one's complement of it & $1F.

### 6.5 Vibrato and portamento (two implementations)

Run only if "vibrato delay" is zero; otherwise decrement the delay and skip vibrato.

**Standard** (module tag `TFMX`): vibrato speed (b27), depth (b28), current position (b29), direction bit
in b2b bit 5 (and bit 0, bit 7 as a toggle/hold control: if bit 7 is set and bit 0 is set the position is
held for this tick). Position moves by speed per tick toward 0 (if direction bit clear) or toward
2*depth/ ... in detail: depth' = depth*2 if depth bit 7 is clear else depth & $7F; if direction bit is
clear: position -= speed, if it goes negative set direction bit and position = 0; else position +=
speed, if it reaches or exceeds depth' set the direction bit clear and position = depth'. Then
delta = position - depth'/2; the note-dependent scaling: let k = 2*n + $A0; if the addition does not
carry out of 8 bits, repeat { delta = delta*2; k += $18 } until it carries. The period becomes p0 +
delta. The bit 0 of b2b toggles each tick. (This mimics the Atari scaling of vibrato by the period.)

**MMME** (tag `MMME`): position moves by speed toward 0 or depth*2 as above (direction bit $20);
delta = position - depth; period = p0 + (delta * p0) >> 10 (arithmetic shift: signed 16x16 product,
shifted right by 8 then by 2).

**Portamento** (row flag bit 5, set by $EA): the 32-bit accumulator adds the signed portamento speed:
standard: speed << 12; the period is p0 minus the top 16 bits of the accumulator. MMME: accumulator
adds the signed 8-bit speed; period = p0 - ((acc(16 bit) * p0) >> 8 >> 2).

The final period is stored as 16 bits into the YM period registers (high byte, low byte) of the voice.
In the MMME path, if the flag byte at voice offset $33 (set by $EB... see table: only `$EB` writes it)
has bit 1 set, the period is forced to 0.

Voice volume: v = (volume - volume reduction (from position flag F) - tracker-wide reduction) & $FF; if
bit 7 set (negative) then 0. Stored as the YM volume byte of the channel. (Volume reduction applies to
all three voices only through the voice's own position flag, so the two byte quantities b30 (from the
position entry F) and b31 (always 0 here) are subtracted.)

### 6.6 SID / "digital voice 3" section (after voice 0, old SID type)

The voice-0 SID mode byte (set by $EE; initial $FF, bit 7 set = disabled) is examined each tick:

* bit 7 set: nothing.
* 0: stop: set mode to $FF and digital state to idle.
* 1: period source = voice 0's computed period.
* 2: source = voice 1's final period; 3: voice 2's final period; 4: volume only; other: voice 0 period.
* For period sources: if the period <= $10 or > $EEE act as stop; else digital Paula period = period*7+1
  (16 bit).
* Then (for any non-stop mode): the sample is one byte taken from the 18-entry level table
  `the level table` indexed by (voice 0's final volume & 15), length 1, loop length 1, volume = the special
  value "take channel A's volume at the Paula conversion step time". Digital state = start.

## 7. Sequencer

Each tick the tracker counter is decremented; when it reaches zero it is reloaded from the tracker speed
and every voice's sequencer step runs (voice 0, 1, 2). Speed values are in ticks.

### 7.1 Row stream (compact pattern encoding in the COSO image)

A pattern is a byte stream: `$FF` = pattern end; `$FE n` = set row speed to n (wait counter reload) and
continue; `$FD n` = set row speed to n and end this voice's step; otherwise a row of:
note byte, flag byte, optional third byte (present when flag & $E0 non-zero).

### 7.2 Voice sequencer step

Decrement the row wait counter (byte). If the result has bit 7 clear (non-negative): done for this
voice. Otherwise reload the wait counter from the row speed and read the pattern stream:

* `$FF`: the position advances: for voice 0 only, the remaining position count is decremented; if it
  becomes negative: **song end** (section 8). Then the next position entry is read (pattern number,
  transpose A, transpose B, flag byte at pointer, then pointer += 12), flag byte high nibble: $F -> voice
  volume reduction = low nibble; $E -> tracker speed = low nibble; the new pattern is loaded and parsing
  continues (this consumes no tick).
* `$FE n`: speed n, continue. `$FD n`: speed n, then stop parsing this tick (pointer is after the byte).
* A row: store note byte (row note) and flag byte; if flag & $E0 non-zero read the third byte as the
  "row parameter" (b2c); clear the portamento accumulator; if the row's note byte has bit 7 set it is a
  **tie**: stop (no new note, existing instrument continues). Else a new note: instrument number =
  (flag & $1F) + transpose B; if that exceeds the instrument count it becomes 0; load the instrument:
  its volume speed and the wait counters, vibrato fields, volume program pointer (volume index 0); pitch
  program number = the instrument's byte 1, or the row parameter when flag bit 6 is set; if it exceeds
  the pitch-program count it is 0; pitch program index 0, wait counters 0.

### 7.3 Position flag byte

High nibble $F: voice volume reduction = low nibble; high nibble $E: tracker speed = low nibble;
others no effect. (The reduction is subtracted from the voice volume, 6.5.)

## 8. Song end

Only voice 0 counts positions. When the voice-0 position counter goes below zero at a pattern end: the
song-end callback is raised, `the emulation reset` is run (YM shadows reset), voice 0's SID byte becomes $FF, the
position counter is reloaded to (end - start), every voice's position offset is reset to 0 relative to
the start position pointer (voice 0 as well), and playback continues at the start. The reference host
stops after the callback.

## 9. YM image to Paula writes (the Paula conversion step)

### 9.1 Order per tick

A, B, C, then S (digital). For each analogue channel the channel logic decides among three cases
and then writes volume and period. Let d7 = ~(mixer shadow) & $3F (bits 0..2 = tone enabled,
bits 3..5 = noise enabled):

* tone enabled for channel idx (bit idx of d7): state 1. If the stored state is not 1: set it, run a
  **note-start sequence** with start = loop = pulse sample, one-shot length 2 words, loop length 2 words.
  Period = ((YM period & $FFF) * 7 + 1) & $FFFF. Volume from the volume table (9.2).
* else noise enabled (bit idx+3): state 2. If the stored state is not 2: set it, note-start sequence with start =
  loop = noise buffer (1024 bytes), one-shot length $200 words, loop length $200 words. Period = the noise
  period table indexed by (shadow noise register & $1F): $280 - $10*i.
* else silent: **always** a note-start sequence with start = loop = the silent sample, lengths 1 and 1; volume 0,
  period $100; state 0.

Then write VOL then PER. (A is Paula channel 0, writes at $A8 then $A6; B is $D8 then $D6; C is $C8 then
$C6.) The voice-A volume is remembered for the digital voice.

Digital voice (S): if state is idle (0): note-start sequence with silent sample, lengths 1 and 1, then PER = the
stored digital period, VOL = stored volume (or channel A's volume when the stored volume is negative =
SID mode). If state is "start" (1): set the state to 2 and note-start sequence with the sample start, loop and the
stored lengths, then PER and VOL as above. If state is 2: write nothing.

### 9.2 Volume table and the volume index

Index = the YM volume byte (0..$3F expected). Values for indices 0..17:
0, 1, 2, 3, 4, 6, 8, 10, 13, 16, 20, 24, 30, 38, 48, 64, 32, 0. For larger indices the table is read
beyond its end and returns bytes of the player's code; the first 52 entries (indices 0..51) are given
in the data file `data/volume-table-overread.bin` (the values for indices 18..51 are instruction
bytes). Real modules do not exceed volume index $3F normally; fuzzed modules showed that values
above $10 occur only with deliberately large volumes. The lead decides on embedding.

### 9.3 note-start sequence (exact write order)

For a channel with Paula block base B, start pointer SA, loop pointer LA, one-shot length L1 and loop
length L2 (words), mask M = 1, 8, 4, 2 for A, B, C, S:

1. B+6 (AUDxPER) = 1.
2. INTREQ ($9C) = (M << 7) if the channel DMA was running, else 0.
3. DMACON ($96) = M (clear) if running, else 0.
4. B+0 (LC) = SA, B+4 (LEN) = L1... (hi/lo long write to B+0 and B+2, then word to B+4).
5. INTREQ ($9C) = M << 7.
6. DMACON ($96) = $8000 | M. Mark the channel running.
7. B+0 (LC) = LA, B+4 (LEN) = L2.

(In the logs every LC write is two word writes B+0 high, B+2 low at the same cc; one cc stamp.)

### 9.4 Constants

Pulse sample: a 4-byte buffer of two words ($80 $7F ... see the bytes in the memory image: the pulse is a
square wave of 2 samples). Noise buffer: 1024 bytes of pseudo-noise located 1024 bytes before the pulse
sample in module memory. Silent sample: 1 word of zero. The sample addresses are host memory choices;
the implementer is free to choose the addresses (the logs show the reference's).

Digital Period: default $248. If the prefix code contains a timer-data word (one of $FA23, $FA1F, $FA25,
$FA21 anywhere in the prefix scan) the byte before the first such word is T; Period = floor($361F10 /
(floor(2457600 / T) >> 2)) + 1 (16-bit divide semantics; if the quotient exceeds 16 bits, the low word of
$361F10 is used; if the first division overflows, 2457600's low 16 bits are used). TypePeriod bits come
from the prefix scan: a specific 4-byte code pattern in the prefix sets bit 1 and another sets bit 0 (the implementer may take the two patterns from the reference logs' module family; absent patterns give 0).

### 9.5 Nibble sample conversion

When the prefix flags equal exactly 1 and the module has samples, the sample area from the first sample
offset to the table's end offset is converted in place before play when its first long has no bit set
in any high nibble: every byte b with bit 7 clear becomes the level table[b & 15]; byte $88 becomes
the level table[17]; any other byte with bit 7 set becomes the level table[16] (= $00).

## 10. Tables

### 10.1 YM period table (index = note, 96 values then zeros)

$EEE,$E17,$D4D,$C8E,$BD9,$B2F,$A8E,$9F7,$967,$8E0,$861,$7E8,$777,$70B,$6A6,$647,$5EC,$597,$547,$4FB,$4B3,$470,$430,$3F4,$3BB,$385,$353,$323,$2F6,$2CB,$2A3,$27D,$259,$238,$218,$1FA,$1DD,$1C2,$1A9,$191,$17B,$165,$151,$13E,$12C,$11C,$10C,$FD,$EE,$E1,$D4,$C8,$BD,$B2,$A8,$9F,$96,$8E,$86,$7E,$77,$70,$6A,$64,$5E,$59,$54,$4F,$4B,$47,$43,$3F,$3B,$38,$35,$32,$2F,$2C,$2A,$27,$25,$23,$21,$1F,$1D,$1C,$1A,$19,$17,$16,$15,$13,$12,$11,$10,$F; indices 96..127 are 0.

### 10.2 the level table (18 bytes)

$80,$93,$A7,$BF,$CF,$DF,$EF,$FF,$0F,$1F,$2F,$3F,$4F,$5F,$6F,$7F,$00,$FF.

### 10.3 Digital note table (96 entries)

entry i = floor($400 * 2^(i/12)). (exact values reproduced by this formula, checked.)

### 10.4 Noise period table

$280 - $10*i for i = 0..31.

## 11. Timing (colour clocks)

All figures are measured on the reference (two real modules over 10 s each, and many synthetic ones).

### 11.1 Tick

The tick entry is 70940 cc after the previous entry (first entry at 345988-ish minus about 1800 for the
first note-start sequence start; the first tick's start is host start-up dependent). The first write of a tick
(the channel-A note-start sequence or, when A needs no note-start sequence, the A volume write) comes after the "entry to first
event" gap: 1790 cc baseline, plus a cost for the Play work. The measured values on the real modules are
1790, 1850, 1906, 1942, 1966, 2034 ... depending on the commands executed (the steps are 8-12 cc per
vibrato calculation, larger for portamento and MMME multiplications); the per-event costs were not fully
resolved (the implementer should use the observed distribution as the placement and accept small
mismatch). The ticks after the first sit exactly 70940 cc apart.

### 11.2 Raster-line waits inside note-start sequence

The note-start sequence is not a pure write sequence: it contains three waits for the video beam (a read of the
beam position register until the line number changes, then until the horizontal position passes 22 in
the same line) before the three critical write groups. The consequence is that the gaps below depend on
the beam phase; treat the measured per-write gaps as follows (cc; first value is typical):

* AUDxPER=1 -> INTREQ: 26. INTREQ -> DMACON: 6.
* DMACON (stop) -> LC: 20 or 21 when the channel was idle (not running); when running the gap depends on
  the Paula channel's period counter (a wait for the channel's audio flag): 20..28500.
* LC -> LEN: 6.
* LEN -> INTREQ: A 195 (idle: A 182..195), B 198 (186..199), C 194, S 189..190 for the second raster
  wait; the first-wait to last-h-read gives INTREQ at 36 or 37 after the last beam read. Median
  values from the tick statistics: A idle 195, B idle 198, C idle 195, S idle 189.
* INTREQ -> DMACON (start): 6.
* DMACON(start) -> LC: 417..432, median 421 (the third wait: 165, 166 or 180 to the first read, then 10
  or 11 after the last read).
* LC -> LEN: 6.
* LEN -> VOL (end of note-start sequence): A tone 162, A noise 189, A silent 181; B tone 222/223, noise 249/250,
  silent 241/242; C tone 205, noise 232; S silent 20, S sample 20 (S then PER immediately).
* VOL -> PER: 12.
* End of A (PER) -> start of B: tone-hold 400, noise-hold 435 or 436; into a B note-start sequence (PER=1) 281
  (tone) / ~442 median; B -> C: 417 or 418 (tone-hold), 453 (noise-hold), 440 median into note-start sequence; C
  PER -> S first write: 232 (when S idle) or 198..457 distribution median 316 (when S does a note-start sequence);
  silent-channel gaps: B after A 223.5, C after B 272.5, noise 246 / 281 as above.
* S PER -> VOL: 243..250 (243.5 typical; 249.5 when the volume comes from the SID source).

The implementer should place each write at its listed gap from the previous write, using the median
values. The beam-poll nature of the reference means exact reproduction needs the host's raster model; the
gaps above have a spread of up to +-12 cc in the real logs.

## 12. Validation summary and unknowns

See README.md. In brief: the write order and values (not the timing) are reproduced for both real
modules (about 500 ticks each) and several hundred synthetic modules by a model built from this text.
