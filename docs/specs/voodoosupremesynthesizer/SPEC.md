# Voodoo Supreme Synthesizer (`vss.*`, `*.vss`) replayer: behaviour spec

*Sanitised for publication: behaviour only.*

Scope: modules of Tomas Partl's Voodoo Supreme Synthesizer (Voodoo Software, 1993) as played by the
DeliTracker player "VSS" (Voodoo Supreme Synthesizer player module V1.5, 1995), which UADE ships as
"VoodooSupremeSynthesizer" (prefix `vss`). The archive copies use the extension form `NAME.vss`.

This text describes behaviour observed in register-write logs and execution traces of the reference 68k
player run through the host, plus the meaning of the module data. It contains no code of the original.
Times are given in 68000 cycles ("cycles") as the reference host charges them, and in colour clocks
("cc"; 2 cycles = 1 cc).

Status (checked by a private model that follows only this text):

| item | result |
|---|---|
| 7 real modules, 10 runs (subsong 0 of each; subsongs 1 to 3 of the one 4-subsong module via a header edit), each from the start to the song end (259 to 12 210 ticks) | every tick write (address, value, order, cycle-exact time from the tick call) and every audio-interrupt write equal the reference: 52 279 ticks, 977 042 tick writes, 1 078 199 interrupts |
| 3 synthetic modules (paths the real modules do not use), 30 s each | same: 4 491 ticks, 84 741 tick writes, 15 061 interrupts |

## 1. File format

All multi-byte values are big-endian. Offsets are byte offsets in the file. The module is loaded as one
block; all pointers are relative and become absolute by adding the address of a base (below).

### 1.1 Header and detection

The header sits at the end of the file:

| off | size | meaning |
|---|---|---|
| 0 | 4 | magic `VSS0` (0x56535330) |
| 4 | u32 | number of subsongs `nsub` |
| 8 | s32 x nsub | for subsong `s`: offset of its song table, relative to the header start |

Detection (all must hold):

- the file size is even;
- the magic is found at one of the 32 even offsets `size - 64 + 2k`, `k = 0 .. 31` (the lowest such
  offset where it matches is the header);
- `nsub < 256` (unsigned).

In the corpus the header is the last 12 bytes (one subsong) or is followed by 4 bytes of junk
(`Qang`, 4 subsongs, header 28 bytes before the end).

### 1.2 Song table and object index

The song table of a subsong (`songtab`) is an array of s32 offsets, each relative to `songtab` itself.
Entry `i` is "object `i`"; `obj(i) = songtab + s32 at songtab + 4*i`. The table has no stored length;
an index byte in the data (0..255) picks an entry.

- Objects 0..3 are the note streams of voices 0..3.
- All other objects are referenced by index from the streams: waveforms, envelopes, wave sequences,
  samples, sub-streams (call and jump targets).

There is no title, author, or play length field. The data before the first object is not used.

Subsongs: `0 .. nsub-1`. The reference host cannot select a subsong (it reads the player's subsong range
before the player has filled it and always shows 0..0); the player itself plays a subsong index that
persists across restarts and is reset to 0 if it is outside `0 .. nsub-1`. A port should offer all
`nsub` subsongs.

### 1.3 Object kinds

| kind | layout |
|---|---|
| waveform | 32 signed 8-bit samples (only the first 32 bytes are read) |
| volume envelope | pairs `(value, count)`, see 4.2 |
| pitch envelope | pairs `(value, count)`, see 4.3 |
| wave sequence | pairs `(speed, count)` (synth mode A, B) or triples `(speed, step, count)` (mode C) or triples `(offset hi, offset lo, count)` (mode D), see 6 |
| sample | signed 8-bit data; the u16 **just before** the object address is its length in bytes |
| stream | bytes, section 3 |

### 1.4 Player tables (not in the module)

Two tables live in the player and are provided as files in `data/`:

- `period.bin`: 96 u16 note periods, index 0 = 0x7E00 down to index 95 = 0x85, 12 per octave (the
  values are not all exact halvings of each other; use the file as it is).
- `ratio.bin`: 13 pairs `(a, b)` of u16 (52 bytes), the frequency ratio `a/b` of 0..12 semitones,
  pair 0 = (1, 1), pair 12 = (2, 1).

A note index (note + transpose, 8 bits) of 96 or more reads past the period table: indices 96..121 read
the ratio table as consecutive words (pair 0's `a`, pair 0's `b`, pair 1's `a`, ...); above 121 the
player's own work memory (unspecified). No real module does this.

## 2. Initialisation

Memory set up by the player when it is loaded: a 512-byte zeroed block of chip memory. Bytes 0x00..0xFF
are four 64-byte synth buffers, voice `v` at `0x40*v`, each made of two 32-byte halves; bytes
0x100..0x1FF are a silent area (zero) called `zero` below.

At (re)start of a subsong, for every voice: all voice state (section 5) is cleared to zero, except

- event counter = 1, wave speed = 1;
- stream pointer = `obj(v)` of the subsong's song table;
- buffer = voice `v`'s synth buffer, current half = 0;
- the subroutine stack is empty (20 long entries per voice, 3.4);
- the volume scale (5) is kept (it is set by the host's volume and balance calls, default 64).

Then these Paula writes, in this order: AUD0LEN..AUD3LEN = 0x0010; AUD0VOL..AUD3VOL = 0;
AUD0LC..AUD3LC = `zero` (each a 32-bit store: high and low word with the same time); DMACON = 0x800F.
When the play interrupt is started: INTENA = 0x8780 (audio interrupts of the four channels on); the
vertical-blank interrupt runs the tick.

Timing of these start-up writes (relative to the start of the call; host business, see 7.4): the first
LEN write at `1746 + floor(49*k/2)` cc where `k` is the header position index of 1.1 (k = 26 for a
header 12 bytes before the end); then +8, +16, +24 (LENs), +32, +40, +48, +56 (VOLs), +192, +208, +224,
+240 (LCs), +248 (DMACON) (+-1 for odd `k`). INTENA comes 269 cc after the start of its call. Paula
raises the four audio interrupts right after DMA starts (7.2).

## 3. The note stream

A voice reads its stream when its event counter runs out. It reads bytes until a note, a portamento or a
rest ends the read; all other commands are executed and the read continues with the next byte.

| byte | operands | action |
|---|---|---|
| `0x00..0x7F` note `n` | `d` | period = `period[(n + transpose) & 0xFF]`; event counter = `d`; then the note tail (3.1). Ends the read. |
| `0xFF` rest | `d` | volume = 0, volume envelope counter = 0, output volume = 0, volume delta = 0; event counter = `d`. Ends the read. Nothing else changes (no tail). |
| `0x81` call | `i` | push the stream position after the operand; continue at `obj(i)`. |
| `0x82` return | | continue at the popped position. |
| `0x83` loop start | `c`, `x` | two operand bytes, the second ignored: push the position after both, then push the count `c`. |
| `0x84` loop end | | pop count and position; count - 1 (32-bit); if it is not 0, continue at that position and push both again; if 0, continue after this byte. A loop with `c` plays its body `c` times; `c = 0` loops 2^32 times. |
| `0x85` waves | `i`, `j` | wave 1 = `obj(i)`, wave 2 = `obj(j)`. |
| `0x86` volume env | `i` | volume envelope = `obj(i)`, position 0, counter 1. |
| `0x87` pitch env | `i` | pitch envelope = `obj(i)`, position 0, counter 1. |
| `0x88` wave seq + mode | `i`, `m` | wave sequence = `obj(i)`, position 0, counter 1; then the mode byte `m` (3.2). |
| `0x89` portamento | `n1`, `n2`, `len` | 3.3. Ends the read. |
| `0x8A` transpose | `t` | transpose = `t` (added to note bytes modulo 256). |
| `0x8B` jump | `i` | continue at `obj(i)`; counts for the song end (7.3). |
| `0x8C` retrigger flags | `f` | flags for the note tail. |
| `0x8D` mask | `k` | the synth "mask" byte (mode A, B) and, in mode D, the period-table index of the reference pitch (6.4). |

Undefined command bytes (`0x80`, `0x8E..0xFE`) make the reference jump to an arbitrary address; they
never occur.

### 3.1 Note tail (after a note or a portamento)

1. Clear the "sample ended" flag (mode bit 1, 6.4); event counter = the duration byte.
2. If flag bit 5 (`0x20`) is clear: wave sequence position = 0, counter = 1, phase = start phase.
3. If flag bit 7 (`0x80`) is clear: volume envelope position = 0, counter = 1.
4. If flag bit 6 (`0x40`) is clear: pitch envelope position = 0, counter = 1.

A note sets the period only; it does not touch DMA or any register.

### 3.2 Mode byte of `0x88`

The full byte becomes the voice mode. Its bits: 6 = mode B, 7 = mode C, 5 = mode D (sample), none =
mode A; when several are set the tick picks B, then C, then D (section 6). Bit 0 = sample pitch
correction (6.4); bit 1 = "sample ended" (normally 0).

- Bit 5 clear: phase = `m & 0x1F`, start phase = the same. If the previous mode had bit 5 set (the voice
  leaves sample mode): AUDxLC = the voice's synth buffer (half 0 address, a 32-bit store), AUDxLEN =
  0x0010, and wave speed = 0 and start phase = 0 (both, overriding the value just set).
- Bit 5 set: AUDxLEN = 0x0040. Phase is not changed.

### 3.3 Portamento `0x89 n1 n2 len`

`p1 = period[(n1 + transpose) & 0xFF]`, `p2 = period[(n2 + transpose) & 0xFF]`; period = `p1`.
`diff = p2 - p1` (16 bit); direction bit `0x80` if it is negative, then `diff = |diff|`.

- step = low byte of `diff / len` (unsigned quotient), 1 if that byte is 0; stored as `direction | step`.
- interval = low byte of `len / diff`, 1 if 0; the interval countdown = 1.
- portamento length = `len`; the event counter = `len` (the third operand is also the duration).

Then the note tail (3.1) with the duration byte = `len`. `len = 0` or `p1 = p2` is a divide by zero in
the reference (a crash); not specified.

### 3.4 Subroutine stack

Calls push one entry, loops two (position, count). The stack holds 20 entries per voice; deeper nesting
overwrites other player memory in the reference (not specified).

## 4. The tick

The tick runs once per video frame (7.4). The four voices are processed in order 0, 1, 2, 3. Per voice, in
this order:

### 4.1 Event counter

Decrement the event counter (8 bit). If it became 0, read the stream (section 3). (A duration of 0 lasts
256 ticks.)

### 4.2 Volume envelope

Decrement the volume envelope counter (8 bit). If it became 0, read the entry at `envelope + position`
(position is 8 bit):

- value `0x88`: position = the next byte; read again at the new position (no limit; a self-loop hangs).
- second byte 0: volume = value; counter = 1; position += 2. The add step below is skipped this tick.
- otherwise: volume delta = value; counter = second byte; position += 2.

Unless skipped: volume = volume + volume delta (8 bit, wraps). The delta stays in force until the next
entry, so a delta entry with count `c` adds its value on `c` consecutive ticks.

### 4.3 Pitch envelope

Decrement the pitch envelope counter (8 bit). If it became 0, read the entry at `envelope + position`:
value `0xFF` = position = next byte and read again; otherwise pitch command = value, counter = second
byte, position += 2.

### 4.4 Portamento

Decrement the portamento length (8 bit). If it became 0: step = 0, interval countdown = 0. Otherwise
decrement the interval countdown (8 bit); if it became 0, reload it with the interval and change the
period by the step: step byte below 0x7F: period += step; 0x7F or above: period -= step & 0x7F (16 bit).

With no portamento active these counters keep wrapping and apply a step of 0, which has no effect but
costs time (section 8).

### 4.5 Pitch command

The pitch command (last pitch envelope value) is applied **every tick**:

| command | effect on the period (16 bit) |
|---|---|
| `0xFE` | doubled (shift left 1) |
| `0x7F` | halved (unsigned shift right 1) |
| `0x7E` | transposition by `n` semitones, `n` = the counter byte just loaded (4.3); then the counter is set to 1, so the next entry follows on the next tick and the transposition happens once per entry. Bit 7 of `n` = down. See below. |
| `0x00..0x7D` | period += command |
| `0x80..0xFD` | period -= command & 0x7F |

Transposition: let `s = n & 0x7F` for down, `n` for up. If `s >= 13`: `q = s / 12`, `s = s % 12`;
shift the period by `q & 7` bits (up: unsigned right, down: left, 16 bit). Then with ratio pair `(a, b) =
ratio[s]`:

- up: result = `period * b / a`, truncated;
- down: result = `period * a / b`, plus 1 if the division leaves a remainder (the remainder test and its cost, 8.2, exist for both directions; for up the addend is 0).

If the quotient does not fit 16 bits, the result is the low 16 bits of the product instead, and "plus 1
if a remainder" uses the high 16 bits of the product as the remainder. The period becomes the 16-bit
result.

### 4.6 Register writes

1. DMACON = `0x8000 | (1 << v)` (every tick, every voice).
2. AUDxPER = the **output period**, AUDxVOL = the **output volume** (both computed on the previous tick;
   0 on the first).
3. New output period = period; new output volume = low byte of `(volume * volume scale) >> 6`.

So the values a tick writes are those of the previous tick's state: periods and volumes reach Paula one
tick late.

### 4.7 Mode work

Section 6, by the mode bits (B if bit 6, else C if bit 7, else D if bit 5, else A).

## 5. Voice state

Per voice: wave 1, wave 2 (pointers; in mode D wave 1 is the sample play position and wave 2 the sample
start); current half (0 or 0x20); phase (5 bit); wave speed (byte); start phase (byte); the mode D step is
the 16-bit value whose high byte is the wave speed and low byte the start phase; buffer pointer; mask;
wave sequence counter; stream position; event counter; volume; period; subroutine stack; volume
envelope pointer, position, counter; pitch envelope pointer, position, counter; pitch command;
portamento step, interval, interval countdown, length; transpose; retrigger flags; wave sequence
pointer and position; mode; mode C step; output period; output volume; volume delta; volume scale.
All counters and positions are 8 bit.

Volume scale: voices 0 and 3 = `(host volume * left balance) >> 6`, voices 1 and 2 = `(host volume *
right balance) >> 6`; 64 with default settings.

Pointers that were never set are 0, and the player reads from machine address 0 upward (an envelope that
was never selected keeps reading memory there every 256 ticks, a wave never set reads 32 bytes at
0). The bytes there are the host's: in the reference host 0..3 are 0, 4..7 hold the exec library base
(0x00002348), and every long from 8 to 0x3FF holds 0x00000404. Two real modules have a voice that never
selects a pitch envelope and audibly read these bytes (the period climbs by 0x23 per tick from tick 1024
on). A port that reads through the emulated machine's memory gets this for free.

## 6. Synth and sample modes

Every mode first steps its wave sequence: decrement the wave sequence counter (8 bit); if it became 0,
read the entry at `sequence + position`. Loop marker: mode A: any first byte with bit 7 set; modes B, C,
D: first byte `0xFF`; position = the next byte and read again. Entry contents and position step:

| mode | entry | position step |
|---|---|---|
| A, B | wave speed, counter | 2 |
| C | wave speed, mode C step, counter | 3 |
| D | sample offset high, low, counter | 3 |

In modes A, B and C the voice then double-buffers: write AUDxLC = buffer + current half (a 32-bit store;
the half filled on the previous tick), flip the current half (xor 0x20), and fill the other half
("target", 32 bytes) as below. Paula plays a 32-byte half (AUDxLEN = 16 words) in a loop and picks up
the new LC at its next loop start. "Previous" below means the half just given to Paula.

### 6.1 Mode A: mix of two waves

Before the buffer flip: phase = (phase + wave speed) & 31. Then for `i = 0..31`:
`s = (wave1[i] + wave2[(phase + i) & 31]) >> 1` (signed, arithmetic, low 8 bits kept). If `s` is
negative (bit 7): `s = -((-s) | mask)`; else `s = s | mask` (8-bit operations; the mask is OR-ed into the
magnitude). target[i] = s.

### 6.2 Mode B: xor window

If wave speed has bit 7: copy wave1[0..31] to the target; nothing else (phase unchanged).

Otherwise a window of `w = speed & 0x1F` bytes starting at the phase, wrapping around 32: target[i] =
previous[i], xor `(wave2[i] & mask)` for `i` inside the window. Precisely: `start = phase`, `end = start
+ w`; if `end >= 32` the "inside" state starts true and `end -= 32`, else false; for each `i = 0..31`
the state toggles when `i == start` and again when `i == end` (both toggles happen if they are equal),
and byte `i` uses the state after the toggles. Then phase = (speed + phase) & 31.

### 6.3 Mode C: morph window

If wave speed is exactly 0x80: copy wave1[0..31] to the target; nothing else.

Otherwise the morph target is wave 1 if `speed & 0xC0 == 0x40`, else wave 2. The window is as in mode B
(`w = speed & 0x1F`). Outside it, target[i] = previous[i]. Inside it, compare in offset binary
(`x ^ 0x80`): `c` = previous byte, `t` = morph target byte, step = the mode C step. If `|t - c| <= step`
the byte becomes the target byte; otherwise it moves by `step` towards it (`c + step` or `c - step`,
converted back from offset binary). Then, unless `speed & 0xC0 == 0xC0`, phase = (speed + phase) & 31.

### 6.4 Mode D: sample

No buffer flip and no LC write in the tick. On a new wave sequence entry: play position (wave 1) =
sample start (wave 2) + 16-bit offset; clear the "sample ended" flag.

Then the step (the advance of the play position per audio interrupt): if mode bit 0 is clear, step =
0x0080 (128 bytes, i.e. plain playback, AUDxLEN = 64 words); if it is set,
`step = floor(128 * period / period[mask]) & 0xFFFF` (`period` = the voice's current period after this tick's pitch command, not the output period) (pitch-corrected: the sample advances at the rate of
the reference period `period[mask]` whatever the played pitch). The step is stored in the wave speed
(high byte) and start phase (low byte) fields.

The audio interrupt of the channel (7.2) feeds Paula: on each interrupt, with `old` = play position,
play position += step; then

- if the "sample ended" flag is set: AUDxLC = `zero`, play position = `zero`;
- else if sample start + length (the u16 before the sample) >= the new play position (unsigned): AUDxLC
  = `old`;
- else: AUDxLC = `zero`, play position = `zero`, set the "sample ended" flag.

The handler tests mode bit 5 alone (not the B/C/D priority of 4.7). Every interrupt (all modes) ends with INTREQ = `0x0080 << channel` (acknowledge). In modes A, B, C the
handler does nothing else. The last block before the end can read up to one block past the sample's end
(module bytes that follow).

## 7. Host behaviour visible in the logs

### 7.1 Tick timing frame

The tick is called once per frame (71 051 cc). An audio interrupt being handled at the frame start delays
the call until the handler returns (tick spacing 70 868 .. 71 234 cc in the logs); interrupts that fall
inside a tick are handled after the tick returns.

### 7.2 Audio interrupts

Paula raises a channel's interrupt each time it (re)starts a block (each loop of the 32-byte half in
modes A/B/C, each 128-byte block in mode D). These are Paula/host events; the replayer only supplies the
handler (6.4) and its timing (8.6).

### 7.3 Song end

A 16-bit song-end word is cleared at the start of each tick; each executed jump (`0x8B`) shifts it left
by one and sets bit 0. If after voice 3 the word equals 15 (exactly four jumps in this tick, e.g. one
per voice), the player signals the song end to the host. Playback itself is not changed (the song keeps
looping); the reference host stops the run at that tick. In the corpus the first song end comes after about 260 to 12 210 ticks.

### 7.4 Start

The absolute time of the first tick and of the start-up writes is host business. Only the times inside a
call (from the call's start) are exact properties of this player.

## 8. Timing

### 8.1 Clock model

Costs are static (no data-dependent instruction timing). For a call, a counter `T` starts at 0 at the
call's start; a register write is stamped `call_time + floor(T / 2)` cc, where `T` includes the cost of
the writing step up to and including that write (the numbers below are positioned accordingly). Both
words of an AUDxLC write have the same stamp.

### 8.2 Tick skeleton

Voice 0 starts at `T = 236`. After a voice ends, `+38` before the next voice starts. (After voice 3 the
remaining costs carry no writes.)

Per voice, steps in the order of section 4:

| step | cycles |
|---|---|
| 4.1 counter | 29; if it ran out: +18, then the stream read (8.3) |
| 4.2 volume envelope | 25; on expiry, per entry looked at: 67, plus 30 for a loop marker (then the next entry, 67 again), else 25 and then 54 (set entry, no add) or 52 (delta entry); the add step costs 28 |
| 4.3 pitch envelope | 29; on expiry, per entry 73, plus 30 for a loop marker, else 52 |
| 4.4 portamento | 29; length ran out: 42; else 29, and if the interval countdown ran out: 55 + 26 (add) or 24 (subtract) |
| 4.5 pitch command | 0xFE: 61; 0x7F: 78; add: 104; subtract: 122; 0x7E: 122 + (25 down / 17 up) + (194 if `s >= 13`) + (64 down / 74 up) + 357 + (4 if a remainder, 4.5) + 22 |
| 4.6 writes | DMACON at +16, AUDxPER at +36 after it, AUDxVOL at +30 after that, then 140 |
| 4.7 mode select | 27 to reach mode B, 54 to reach C, 81 to reach D or A |

### 8.3 Stream read

16 at the start; each stream byte read costs 33, then:

- note: 96, then the note tail;
- command byte (`0x80..0xFF`): 25, then rest: 74 + 32 (end of the read); any other command: 24 + 22 x
  (`cmd - 0x80`), then the command cost below (write times in the table are counted from the start of that command cost), then the next byte (33 ...):

| cmd | cycles |
|---|---|
| 0x81 call | 200 |
| 0x82 return | 54 |
| 0x83 loop start | 92 |
| 0x84 loop end | 75, plus 54 when it loops back |
| 0x85 waves | 386 |
| 0x86, 0x87 envelopes | 232 |
| 0x88, mode bit 5 clear, previous mode not sample | 370 |
| 0x88, mode bit 5 clear, previous mode sample | AUDxLC at 384, AUDxLEN at 400, total 426 |
| 0x88, mode bit 5 set | AUDxLEN at 305, total 315 |
| 0x89 portamento | 227 (+12 if downward) + 163 (+8 if the step byte was 0) + 185 (+16 if the interval byte was 0) + 38, then the note tail; the read ends |
| 0x8A, 0x8C, 0x8D | 26 |
| 0x8B jump | 232 |

Note tail: 36 + 25 (+72 if the wave sequence restarts) + 25 (+32 volume envelope restart) + 25 (+32 pitch
envelope restart) + 32 (end of the read).

### 8.4 Mode work

Wave sequence step (all modes): 25; on expiry, per entry looked at: 69 in mode A, 67 in B, C, D; plus 30
for a loop marker; the accepted entry then costs 48 (A), 52 (B), 72 (C), 136 (D).

| mode | cycles after the wave sequence step (a copy's 740 is the whole rest of the voice) |
|---|---|
| A | AUDxLC at +114; then 184; per byte 87 (result non-negative) or 105 (negative); then 20 + 16 |
| B | AUDxLC at +82; then 83; copy (speed bit 7): 740. Otherwise 75 (+16 if the window wraps) + 4; per byte 90, +8 per toggle, +24 inside the window; then 60 |
| C | AUDxLC at +82; then 99; copy (speed 0x80): 740. Otherwise 41 (+12 when wave 1 is the target) + 59 (+16 if the window wraps) + 4; per byte 90, +8 per toggle, inside the window +37 and then: target below the byte (offset binary) 35 if reached / 39 if stepped, target at or above 31 if reached / 25 if stepped; then 41 (+32 if the phase advances) + 16 |
| D | 25, then 32 (step 0x80) or 504 (pitch-corrected step) |

The voice ends there.

### 8.5 Checks against the logs (cc)

When the wave sequence is not read in that tick, a voice's AUDxLC write follows its AUDxVOL write by
180 cc in mode A, 137 cc in mode B and 150 or 151 cc in mode C (the sums of 8.2 and 8.4). AUDxPER
follows DMACON by 18 cc and AUDxVOL follows AUDxPER by 15 cc, always.

### 8.6 Audio interrupt handler

Times from the handler call:

| case | writes |
|---|---|
| mode not D | INTREQ at 353 cycles (176 cc); returns at 369 (184 cc) |
| mode D, "ended" flag already set | AUDxLC at 350 |
| mode D, still playing | AUDxLC at 377 |
| mode D, end reached now | AUDxLC at 419 |

In mode D, INTREQ follows the AUDxLC write by 168 cycles and the handler returns 16 cycles after that.

## 9. How to check

`tools/replay-cmp.py REF.log PORT.log` (in the repo) compares logs tick by tick. LOGS.md lists the
references. The player's synth buffers and silent area are its own memory: their addresses differ from
a port's by a constant, which the tool accepts.
