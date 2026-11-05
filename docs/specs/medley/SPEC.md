# Medley (Paul van der Valk, `mso.*`) replayer: behaviour spec

*Sanitised for publication: behaviour only.*

Scope: the Amiga "Medley" music format by Paul van der Valk (1990-1991 game music; Eagleplayer/UADE
player name "Medley", file name prefix `mso.`), as played by the UADE player "Medley" (V0.1 for
UADE, 2001). The module holds only data; the replay routine lives in the player. This text
describes the observed behaviour of that player in my own words: file layout, the per-tick state
machine, the exact Paula register writes (value, order) and their timing in 68000 cycles. It
contains no code of the original.

Numbers: "cc" = Amiga colour clocks, "cycles" = 68000 CPU cycles (2 cycles = 1 cc). All
multi-byte values are big-endian. "byte add" means 8-bit wrap-around arithmetic, "word" 16-bit.

Status: a private model that follows this text reproduces every register write (address, value,
order) and its colour clock (relative to the tick call) of 20 reference runs (7 synthetic modules,
about 150 000 writes, 8 700 ticks) exactly; see LOGS.md. No real module of the format was
available, so all checks use synthetic modules built from this description (README).

Two data blocks of the player are needed and are not in the module: the period table
(`data/periods.bin`) and the detune table (`data/detune.bin`); their structure is in 4.2 and 4.3.

## 1. File format

### 1.1 Header and tables

| offset | size | meaning |
|---|---|---|
| 0 | 4 | magic `MSOB` (0x4D534F42) |
| 4 | long | self-relative offset of the **song table** (table = 4 + value) |
| 8 | long | self-relative offset of the **sequence table** (table = 8 + value) |
| 12 | long | self-relative offset of the **instrument table** (table = 12 + value) |
| 16 | long | self-relative offset of the **waveform table** (table = 16 + value) |

"Self-relative": the target is the address of the long itself plus its signed value.

Each table is an array of longs, entry `i` at table + 4*i. An entry is again self-relative:
object `i` = (address of entry `i`) + (entry value). Value 0 means "no object" where the player
checks it (stated below); where it does not check, 0 simply yields the entry's own address.
The word just before a table (table - 2) holds a count: for the song table it is the number of
songs (its low byte is also the highest valid song index); for the other tables it is not used by
the replay.

Every object (song, sequence, instrument, waveform) starts with a 16-byte area the replay never
reads (presumably a name); the fields below are given from the object start.

### 1.2 Song (one per subsong)

| offset | size | field |
|---|---|---|
| 0x10..0x13 | 4 bytes | track sequence number for voice 0, 1, 2, 3; 0 = voice unused |
| 0x18 | word | tempo value (stored, never used by this player: the tick rate is fixed, 3.1) |
| 0x1A..0x1D | 4 bytes | start instrument for voice 0..3; 0 = use the default |
| 0x1E | byte | default instrument (if 0 too: instrument 1) |
| 0x1F | byte | global transpose (added to every note, byte add) |
| 0x20 | byte | speed: output divider, see 2.4 |
| 0x21 | byte | stored, unused |
| 0x22 | byte | detune pattern number (4.3) |
| 0x23 | byte | detune step interval in ticks (0 = detune pattern never steps) |
| 0x24 | byte | master volume 0..63 (only the low 6 bits are used) |
| 0x26..0x29 | 4 bytes | random-trigger base delay for voice 0..3 (0 = normal voice) (2.7) |
| 0x2A..0x2D | 4 bytes | random-trigger delay range for voice 0..3 (2.7) |
| 0x2E..0x31 | 4 bytes | voice volume 0..63 for voice 0..3 (low 6 bits used) |

### 1.3 Sequence

Byte stream from offset 0x10 of the sequence object. Two-byte elements:

- `n d` with `n` < 0x80: an event. `n` = 0 is a rest (key off), `n` = 1..0x7F a note. `d` =
  duration in ticks; `d` = 0 means "duration 0: read the next element at once"; bit 7 of `d` set
  means "tie": the note is not re-triggered (the duration is `d` AND 0x7F).
- `c a` with `c` >= 0x80: a command `c` AND 0x0F with argument byte `a` (2.3). The upper bits of
  `c` other than bit 7 are ignored.

### 1.4 Instrument (fields from the object start)

| offset | size | field |
|---|---|---|
| 0x10 | byte | type: 0 looped waveform, 1 windowed sample, 2 synth (two-oscillator mix), 3..255 one-shot |
| 0x11 | byte | window offset base (type 1) |
| 0x12 | byte | oscillator phase base (type 2) |
| 0x13 | byte | instrument transpose (byte add) |
| 0x14..0x1B | 8 bytes | waveform slots 0..7 (slot 0 is the waveform of a type 2 instrument) |
| 0x1C | byte | type 2: oscillator step byte (high nibble: step of oscillator 1, low nibble: oscillator 2) |
| 0x28 | word | envelope peak level |
| 0x2A | word | envelope sustain level |
| 0x2C | word | attack rate (added per output tick) |
| 0x2E | word | decay rate |
| 0x30 | word | release rate |
| 0x32 + 18*k | 18 bytes | modulator k, k = 0..3 (2.6) |

Modulator record (18 bytes):

| offset | size | field |
|---|---|---|
| 0 | byte | target: 0 off, 1 vibrato, 2 tremolo, 3 window offset, 4 oscillator phase, 5 pitch up, 6 pitch down, 7..255 oscillator step |
| 1 | byte | square flag (output is 0 or amplitude) |
| 2 | byte | free-running flag (not reset by a note trigger) |
| 3 | byte | one-shot flag (stops at the bottom instead of cycling) |
| 4 | byte | unused |
| 5 | byte | invert flag (output = amplitude - value) |
| 6 | byte | delay in output ticks after a trigger |
| 7 | byte | start-at-top selector (2.6) |
| 8 | byte | start-at-middle selector (2.6) |
| 9..11 | 3 bytes | unused |
| 12 | word | amplitude |
| 14 | word | rise rate |
| 16 | word | fall rate |

### 1.5 Waveform

| offset | size | field |
|---|---|---|
| 0x10 | word | length L in bytes (types 0, 3: played length; type 1: see 2.8; type 2: must be a power of two) |
| 0x14 | byte | octave of the recording `o` (lowers the note by 12*o; type 1: also the window shift) |
| 0x16 | byte | type 1 only: "second half" flag |
| 0x18 | L bytes | signed 8-bit sample data (type 1 may read up to 2L bytes, 2.8) |

### 1.6 Detection, subsongs, title, length

- Detection: the first four bytes are `MSOB`. Nothing else is checked. No other UADE/Eagleplayer
  format uses the `mso` prefix.
- Subsongs: numbered 1..N, N = the word before the song table. Song table entry 0 is never played.
  A subsong whose number is larger than the low byte of that word, or whose entry is 0, does not
  start (see 3.2).
- No title, author or length fields are read (the player's own info names Paul van der Valk as the
  author of the replay). Songs loop forever; the only end signal is 2.3 command 0.

## 2. Replay

### 2.1 Per-voice state (all zero when the player is loaded)

active flag; note (byte); gate flag; trigger flag; envelope level (word) and envelope phase
(attack or decay); duration counter (byte); sequence pointer; track start pointer; new-row flag;
speed counter (byte); current instrument; octave (byte); current waveform; base period; period
(word); output volume level (word); window offset, phase offset (words), step offset (byte);
modulator state k = 0..3: value (word), delay counter (byte), direction (0 rising, 1 falling);
synth buffer valid flag, last phase (word, starts 0), synth length, synth shift; detune (signed
word); voice volume and its 64-entry volume table; voice transpose (byte); random base, random
range, random counter (bytes); call stack (up to 8 return pointers) and loop stack (up to 8
pointer+count pairs). Fixed per channel: "right" flag (true for channels 1 and 2), "upper" flag
(true for channels 2 and 3).

Global: tick counter (word, counts every tick), random seed (word), master volume table, speed,
global transpose, detune pattern pointer/start, detune interval and counter.

State survives a subsong change except the fields a start resets (3.2).

### 2.2 One tick

1. Tick counter += 1 (always, wraps at 16 bits).
2. Detune step: if the detune interval is not 0: counter -= 1 (byte); when it reaches 0: reload it
   with the interval; if the byte at the pattern pointer is 0x80, the pointer goes back to the
   pattern start; then the four signed bytes at the pointer become the detune of voices 0..3 (sign
   extended), and the pointer advances by 4. (The pattern start row is therefore applied at the song
   start and again at the first step.)
3. Voices 0, 1, 2, 3 in this order (2.2.1).

#### 2.2.1 One voice

- Inactive voice: random trigger (2.7), nothing else.
- Active voice:
  1. duration counter -= 1 (byte); if it becomes 0: read a row (2.3): new-row flag on, speed
     counter = 0, then read elements.
  2. Speed counter -= 1 (byte). If the result has bit 7 clear, the voice does nothing more this
     tick (no writes). Otherwise the counter is reloaded with the song's speed and the voice
     produces output (2.4). With speed s < 0x80 a voice outputs on its row tick and every s+1 ticks
     after it.

### 2.3 Reading a row

Repeat:
- read an element (1.3);
- command: execute it (table below) and continue with the next element (unless stated);
- event: if `n` > 0: note = `n`, gate on, trigger on; if `n` = 0: gate off, trigger off (the note
  is kept). Duration `d`: if `d` = 0 read the next element (the flags just set stay unless the next
  event changes them); else if bit 7 is set: trigger off; duration counter = `d` AND 0x7F; stop
  reading. (A duration byte of 0x80 leaves the counter at 0, giving 256 ticks.)

| cmd | argument `a` | effect |
|---|---|---|
| 0 | ignored | if the call stack is not empty: pop, continue there. Else, normal voice: continue at the track start and signal **song end** to the host (every time any voice wraps). Else (random-triggered voice, 2.7): voice inactive, DMACON = channel bit (DMA off), the voice's tick ends here |
| 1, 9..15 | ignored | nothing |
| 2 | v | voice volume = (v >> 1) AND 63, rebuild the voice volume table (2.5) |
| 3 | i | if instrument table entry `i` is not 0: current instrument = instrument `i` |
| 4 | m | random jump: if m = 0 nothing; else idx = rnd() AND (m-1), s = the byte at (position after the argument) + 2*idx + 1; if sequence table entry s is 0: nothing; else continue at (sequence table start + entry value) — note: relative to the table start, not to the entry, and without skipping a 16-byte header (a quirk; only meaningful for specially placed data) |
| 5 | s | call: if sequence table entry s is 0, or 8 calls are already stacked: nothing; else push the position after the argument and continue at sequence s (offset 0x10) |
| 6 | n | loop start: if fewer than 8 loops are stacked push (position after the argument, n); else nothing |
| 7 | ignored | loop end: if the loop stack is empty nothing; else count -= 1 (byte); if 0: pop and continue; else continue at the loop's position. Loops and calls use separate stacks, so a loop end inside a called sequence acts on the caller's loop |
| 8 | t | voice transpose = t |

rnd(): seed = (seed + tick counter) AND 0xFFFF; result = seed >> 4.

### 2.4 Output tick

If the new-row flag is set, clear it and compute the note setup (2.4.1). Then, every output tick:
period = base period; envelope (2.4.2); modulators (2.6); waveform output (2.8); register writes
(2.9).

#### 2.4.1 Note setup (on every row, also for rests and ties)

x = note + global transpose + voice transpose + instrument transpose (byte add).

- Type 2: octave = x div 12 (unsigned), semitone = x mod 12; base period = P[12 + semitone];
  waveform = waveform slot 0; synth buffer valid = false.
- Other types: octave group k = number of thresholds 0x1A, 0x26, 0x32, 0x3E, 0x4A, 0x56, 0x62 that
  x is >= (0..7); octave = k; take waveform slot k; while it is 0 and the slot number > 0 take the
  next lower slot (slot 0 is taken whatever it holds; a 0 there selects the waveform table's entry 0
  itself, see 1.1). Then x = x - 12*o (o = the waveform's octave byte; only 0..7 are meaningful);
  type 1: x = x - 12; x = x + 0x30; base period = P[x] (byte add throughout).

P = the period table (4.2). For x beyond its 110 entries the player reads on into the detune
table (indices 110..153 = the 44 big-endian words of `data/detune.bin`); beyond that the value is
undefined.

#### 2.4.2 Envelope

e = envelope level. If trigger: e = 0, phase = attack. Then:
- gate on, attack: e += attack rate; if that carries past 0xFFFF or e >= peak: e = peak, phase =
  decay.
- gate on, decay: e -= decay rate; if that borrows or e <= sustain: e = sustain.
- gate off: e -= release rate, at least 0.

Store e; output volume level = e; window offset = phase offset = step offset = 0.

### 2.5 Volume tables

A 64-entry table for value v (0..63): t[0] = 0; t[i] = i if v = 63, else 1 + floor(i*v/63). The
master table uses the song's master volume, each voice table its voice volume (song start: song
byte; command 2). Volume written = master[ voice[ (high byte of the output volume level) >> 2 ] ].

### 2.6 Modulators (k = 0..3 in order, skipped when the target byte is 0)

With amplitude A, value x (from the state):
1. On a trigger: delay counter = delay. Then, only if the modulator is not free-running (a
   free-running one skips all of this, including the middle start): direction = rising, x = 0;
   if the start-at-top selector is not 0 and the selected flag (2.6.1) is set: direction =
   falling, x = A; then if (target = 1 and delay != 0), or (start-at-middle selector != 0 and its
   flag is set): x = A >> 1.
   Step 2 follows in the same tick (a delay counter loaded here is already decremented once).
2. If the delay counter is not 0: decrement it, x unchanged.
   Else rising: x += rise rate; if that carries or x >= A: x = A, direction = falling.
   Else falling: x -= fall rate; if the result is 0 or borrows: x = 0 (if it borrowed) and,
   unless one-shot, direction = rising.
3. Store x. Output o = x; if square: o = A when falling, 0 when rising; if invert: o = A - o.
4. Apply: 1 vibrato: period += ((A >> 1) - o) arithmetic-shifted right by 8 (signed word);
   2 tremolo: output volume level -= o, at least 0; 3: window offset += o >> 8; 4: phase offset +=
   o >> 8; 5: period += o >> 5; 6: period -= o >> 5; 7 and above: step offset += o >> 8 (byte).
   (Word arithmetic, unsigned shifts unless stated.)

#### 2.6.1 Selector flags

Selector 1 = the channel's "right" flag (channels 1 and 2), 2 = the "upper" flag (channels 2 and
3). Other non-zero values read unrelated voice state and are undefined.

### 2.7 Random-triggered voices

A voice whose song random base is not 0 starts inactive and never reads its track as a
sequence; the trigger reads m and the pairs from the track start (its track must start with a command 4
element `0x84 m` followed by m pairs `0x85 s`; m should be a power of two, 0 is undefined).
Every tick it is inactive: if (tick counter AND 3) = 0: if its random counter is 0, counter =
((rnd() AND (range - 1)) + base) (byte add; the first rnd() call of the tick); counter -= 1; if it became 0: idx = rnd() AND (m - 1),
s = the second byte of pair idx; if sequence table entry s is not 0: sequence pointer = sequence s
(offset 0x10), duration counter = 1, voice active (it reads its first row on the next tick). When
that sequence ends (command 0 with an empty call stack) the voice goes inactive again (2.3).

### 2.8 Waveform output (sample address A and length N in words)

W = waveform data (offset 0x18), L = its length byte count.
- Type 0: A = W, N = L/2.
- Type 1: N = L/4. y = window offset base + window offset (word); the low byte of y is shifted right
  by the waveform's octave byte (8 or more gives 0), the high byte is kept; if bit 0 of y is set and
  the waveform's second-half flag is not 0: A = W + L, else A = W; then A += y AND (L/2 - 1) with bit
  0 cleared.
- Type 2 (synth): phase = phase base + phase offset (word). If phase equals the last phase and the
  buffer is valid, reuse the buffer. Else last phase = phase and regenerate the voice's 32-byte
  buffer: step = (step byte + step offset) (byte); s1 = high nibble (0 means 16), s2 = low nibble (0
  means 16); size = 32 bytes; shift = octave if octave < 4, else 4 with size >>= min(octave-4, 4)
  and s1, s2 <<= that same amount. For i = 0..size-1: p1 and p2 (p1 starts at phase, p2 at 0) are
  ANDed with L-1, byte i = sample[p1] + sample[p2] (byte add), then p1 += s1, p2 += s2. Buffer valid;
  synth length = size/2 words. Every output tick (regenerated or not): period = period >> shift
  (shift and length from the last regeneration); A = the voice's buffer, N = synth length.
- Types 3..255: on a trigger tick as type 0; otherwise A = the 4-byte silence block, N = 2.

The silence block and the four 32-byte synth buffers are player memory: silence, then buffer 0,
1, 2, 3 contiguous (buffer c = silence + 4 + 32*c); contents of the silence block are 0.

### 2.9 Register writes of an output tick (per voice, exactly this order)

1. AUDxLC = A (32-bit, both words at the same cc)
2. AUDxLEN = N
3. AUDxPER = period + detune (word)
4. AUDxVOL = volume (2.5), written as a byte: the log shows v * 0x0101
5. DMACON = 0x8000 | (1 << channel)
Then the trigger flag is cleared. DMA is never switched off by a note; a new sample start takes
effect when Paula reloads the location at the end of the current loop.

## 3. Host interface

### 3.1 Timing frame

The play routine is called from the host's 50 Hz timer: one tick every 70 940 cc, on a fixed grid
(the song's tempo word has no effect).

### 3.2 Load and subsong start

- On load: DMACON = 0x0001, 0x0002, 0x0004, 0x0008 (about 43 cc apart). Voice state, tick counter
  and seed are zero.
- Every subsong start (also when switching): DMACON = 0x0001..0x0008 (all four, about 43 cc apart);
  then, if the subsong is valid (1.6), for voice 0..3: active = gate = trigger = false, speed counter
  = random counter = voice transpose = 0, duration counter = 1, call and loop stacks emptied, detune
  = 0, envelope level = 0, all modulator states zero, DMACON = channel bit, voice volume 63; then if
  the voice's track number is not 0: sequence pointer = track start = sequence (offset 0x10),
  instrument = song's instrument (1.2 rule; the entry is not checked for 0), random base and range
  from the song, active = (random base is 0), master table from the master volume, speed, global
  transpose, detune pattern (4.3; a pattern number above the highest one selects pattern 0): the
  pattern start row is written to the detune of all four voices, detune counter = 1, voice volume
  from the song. Because this happens during the set-up of each voice that has a track, a voice
  set up later without a track keeps the detune 0 its own reset gave it. A voice with track 0
  stays inactive and silent. An invalid subsong performs none of this (only the first four
  DMACON writes) but the player still plays (all voices inactive unless left active from an
  earlier song).
  The reset DMACON writes of the four voices are about 5 000-5 100 cc apart (host-dependent,
  not modelled).
- Tick counter and seed are not reset by a subsong start.

## 4. Data

### 4.1 Note range

For non-synth types the useful range keeps x + 0x30 - 12*o (- 12 for type 1) within 0..109.

### 4.2 Period table

`data/periods.bin`: 110 big-endian words P[0..109], one semitone per entry, from 15360 (index 0)
down to 28 (index 109); P[48] = 960, P[50] = 856, P[60] = 480. Synth instruments use P[12..23]
(7680 down to 4064) and shift the result right by the octave (2.8).

### 4.3 Detune table

`data/detune.bin` (88 bytes): byte 1 = highest pattern number (5); the word at 2 + 2*i is the
signed offset of pattern i relative to that word; a pattern is a list of 4-byte rows (signed
detune of voices 0..3) ended by a row whose first byte is 0x80. Pattern 0 is a single zero row.

## 5. Timing

### 5.1 Clock model

All costs are 68000 cycles and static (no data-dependent instruction times; loops cost per
iteration as given). For a tick, a counter T starts at 0 when the host calls the tick. A register
write gets the colour clock `call + floor(T / 2)` where T includes the cost listed for that write.
The tick call times are 70 940 cc apart. One exception: the song-end signal (2.3 command 0) restarts
the half-cycle count: at the point marked in 5.4, T is rounded down to an even number.

### 5.2 Tick skeleton

| step | cycles |
|---|---|
| tick entry (always) | 344 |
| detune test | 21 |
| detune interval not 0: count | +29 |
| detune step taken | +319 (+16 more if the pattern wrapped) |
| per voice: call | 26 |
| per voice: return (after any voice, except a random voice that ended, 2.3 cmd 0) | 16 |

### 5.3 Voice body

Inactive voice: 44; random base 0: done. Random voice: +29; not a 4th tick: done. Else +21; if
the random counter is 0: +88 +44; then +25 (decrement); not 0: done. Else +205 (pick); entry 0:
done; else +68.

Active voice: 85. Row read: +48, then per element: 21 for the fetch, then

- command: 64 + handler (5.4) + 10 (the 64 applies to every command; the 10 does not follow a
  random voice's end, 5.4);
- note (`n` > 0): 63, rest: 41; then duration 25; duration 0: next element; otherwise the row
  ends with +35 for a plain duration or +35 +36 = 71 for a tie (bit 7 set, also on a rest).

Then speed test 27; no output: done. Output: +43, then:

New row only (no cost at all on other output ticks), note setup:
- 89; type 2: + 8 + 17*(octave + 1) + 212 (total 309 + 17*octave);
- other: + 89 + slot search + 183 (+8 for type 1) + 78. Slot search, starting at slot k: 19 for
  each slot read from k down to the slot found when that slot is > 0 (19*(k - j + 1) for found
  slot j > 0); when it falls through to slot 0: 19*k + 10.

Envelope: 53; trigger +20; then +21; then
| case | cycles |
|---|---|
| attack, below peak | 63 |
| attack, reached peak | 101 |
| attack, carried past 0xFFFF (checked first) | 80 |
| decay, above sustain | 63 |
| decay, at/below sustain | 85 |
| decay, borrowed | 64 |
| release | 21 (+4 when it borrowed and was clamped to 0) |
then 92.

Modulators, each k: target 0: 19 + 30. Else 48, then:
- trigger: +41. If not free-running: +45; start-at-top selector != 0: +23 (+28 more if its flag
  is set); target test +21; for target 1 also +21 (delay test), and when the delay is not 0 the
  middle start applies without the next test; in every other case the middle selector test +21,
  and when the selector is not 0 +23 (its flag test); when the middle start applies: +20.
- delay test 21; delay counter not 0: +16. Else +21 and: rising, not at the top: 42; reached A: 80;
  carried: 59. Falling, above 0: 30; exactly 0: 21; borrowed: 34; at the bottom (0 or borrowed):
  +21, +26 more when not one-shot.
- store 29; square: +25 (+12 when falling); invert test 21 (+16 when set).
- apply: 1: 83; 2: 55 (81 when clamped at 0); 3: 99; 4: 108; 5: 123; 6: 132; 7+: 138.
- next: 30.

Waveform output: 67, then
- type 0: 22;
- type 1: 106; bit 0 set: +21 (+8 when the second half is used); then 24;
- type 2: 79; phase equal to the last phase: +21 (buffer valid: skip the regeneration);
  regeneration: 117; +4 per zero nibble; when octave >= 4: +63 (and +4 more when octave > 8);
  then, for every regeneration: +88, +65 per buffer byte, +16; then (regenerated or not) 88;
- types 3+: 51; trigger: +22; else +30.

Writes: 49, then LC at +28, LEN at +12, PER at +36, VOL at +70, DMACON at +24 (each value is
the cycles from the previous point to that write, the write included); then 16 (clear trigger).

### 5.4 Command handlers (cycles, each including its return)

| command | case | cycles |
|---|---|---|
| 0 | call stack not empty (return) | 79 |
| 0 | normal voice wraps | 67 + 218, **re-base** (5.1), 156 |
| 0 | random voice ends | 67 + 53, DMACON write (+24), 28; the voice ends (no 10, no voice return) |
| 1, 9..15 | | 16 |
| 2 | | 118 + 35*64 + 8*c + 80, c = 64 for v = 63 else 1 + floor(64*v/63) (v = new voice volume) |
| 3 | entry 0 | 79 |
| 3 | instrument set | 119 |
| 4 | m = 0 | 33 |
| 4 | entry 0 | 192 + 16 |
| 4 | jump | 192 + 28 |
| 5 | entry 0 (checked before the stack) | 79 |
| 5 | stack full | 128 |
| 5 | call | 174 |
| 6 | pushed | 109 |
| 6 | stack full | 45 |
| 7 | no loop | 37 |
| 7 | loop back | 98 |
| 7 | loop done | 88 |
| 8 | | 28 |

### 5.5 Example

First tick, voice 0, type 0 instrument, note 0x30 (octave group 2, waveform in slot 1), no
detune, all modulators off: 344 + 21 + 26 + 85 + 48 + 21 + 63 + 25 + 35 + 27 + 43 + 89 + 89 + 38 +
183 + 78 + 53 + 20 + 21 + 63 + 92 + 4*49 + 67 + 22 + 49 + 28 = 1826 cycles: AUDxLC at call + 913 cc,
LEN + 919, PER + 937, VOL + 972, DMACON + 984.
