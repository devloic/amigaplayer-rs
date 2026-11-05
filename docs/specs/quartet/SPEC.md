# Quartet (Amiga) replayer: behaviour spec

*Sanitised for publication: behaviour only.*

Scope: the Amiga music format of the "Quartet" editor (Dan Lennard, 1990), file name prefix `qpa.`
(module) with a companion sample "set" file, as played by the reference (the Eagleplayer "Quartet",
Wanted Team adaptation V1.0). The Atari ST variants (Quartet ST, Quartet PSG) are different formats and
are not covered. This is a description of observed behaviour (register-write logs, beam-read traces,
module data) in my own words. Numbers in "cc" are Amiga colour clocks; "cycles" are CPU cycles of the
reference host (1 cc = 2 cycles).

A model written from this description reproduces the reference's register writes (address, value,
order and colour-clock stamp) exactly for 36 synthetic modules (48 712 writes), including the
clock of the first tick. No real Quartet module was available (see README).

## 1. Files

### 1.1 Module (`qpa.NAME`)

Big-endian bytes.

| offset | size | meaning |
|---|---|---|
| 0 | byte | tempo: ticks per row (1..30, a divisor of 3000) |
| 1 | byte | 0x50 (format mark) |
| 2 | 8 per row | rows: four 2-byte channel entries (channel 0, 1, 2, 3) |
| end | 8 | four words 0xFFFF (end mark), then up to 15 zero words of padding |

A channel entry is two bytes b0, b1:

| bits | meaning |
|---|---|
| b0 bits 7-6 | octave code o (0..3) |
| b0 bits 5-0 | volume - 1 (volume 1..64) |
| b1 bits 7-4 | sample number s (0..15) |
| b1 bits 3-0 | note n (1..12 normal; 0 and 13..15 see 4.3) |

- b0 = 0: no event on that channel in that row (whatever b1 is).
- b1 = 0xFE: slide marker (no event on that channel; see 4.4).
- A row any of whose four words is 0xFFFF is the end of the song (4.5).

### 1.2 Detection

Accepted when all of these hold:
- byte 1 is 0x50; byte 0 is 1..30 and 3000 is divisible by it (1, 2, 3, 4, 5, 6, 8, 10, 12, 15, 20,
  24, 25, 30);
- reading 16-bit words backwards from the end of the file (file size rounded down to even): at most 15
  zero words, then a word 0xFFFF, and the three words before that one are 0xFFFF as well (so the last
  non-zero data is 8 bytes of 0xFF).

### 1.3 Sample set (`SMP.NAME`, else `SMP.set`)

Loading: the companion's name is the module's file name with its first three characters replaced by
`SMP` (the module's name must start with `QPA.`, each of the three letters in either case); the file
is looked for in the module's directory. If it cannot be loaded, the file `SMP.set` in the same
directory is tried. Neither: the module fails to load.

Set layout (size Z bytes, big-endian):

| offset | size | meaning |
|---|---|---|
| 4 | 4 | ASCII `lock` (the header reads "...Block") |
| 16 + 16 i | 16 | name of sample i (i = 0..15), display only |
| 336 | word | loop word: bit i set = sample i loops |
| 354 | ... | sample data, signed 8-bit, stored one after the other |
| Z - 96 + 2 i | word | length of sample i in words (i = 0..15) |
| Z - 64 + 4 i | word | start offset of sample i in bytes |
| Z - 62 + 4 i | word | special length of sample i in words (0 = none) |

Checks (else the module does not load): Z > 450; Z even; `lock` at 4; and
Z = 450 + 2 * (sum over samples with a non-zero length of (length + 2)) - 2.

Sample addresses (A = load address of the set + 354, processed for i = 0..15 in order):
- length > 50: sample i starts at A, its length is its length word; then A += (length + 2) * 2;
- length <= 50 (including 0): sample i starts at A, its length is 50 words, and A does NOT advance (the
  next sample starts at the same address). This is how the reference behaves even though the size check
  above counts these samples.

### 1.4 Song length, position, info

- Song size: scan the module in 4-byte steps starting at offset 2 for the first long equal to
  0xFFFFFFFF; with E = offset just after that long, song size = E + 4.
- Length (in 32-row blocks) = ((song size - 2) >> 8) + 1. It decides looping (4.5).
- Position shown = (row offset from offset 2) >> 8, i.e. row / 32.
- There is no title or author string; one subsong. Sample names come from the set (1.3).

## 2. Hardware written

Only the four audio channels' AUDxLC (written as one 32-bit write = two 16-bit writes, high word
first, same clock), AUDxLEN, AUDxPER, AUDxVOL, and DMACON. No interrupt registers, no ADKCON, and
nothing at initialisation. The reference writes nothing before the first row.

Volume written: hw = (v * G) >> 6 where v is the volume 1..64 and G the host gain (64 by default:
channels 0 and 3 use the left gain, 1 and 2 the right gain; gain = balance * master volume / 64). A voice
switched off by the host has its product masked to 0 before the shift, so it writes 0. Default: hw = v.

## 3. State

Per channel c (0..3): period P (starts 288), volume, sample address and length of the current note, the
"loop" address/length to write after the note's start (4.2), a slide flag (starts clear) and the slide
values (4.4). Global: row pointer (starts at offset 2), tick counter (starts at tempo).

## 4. Replay

### 4.1 One tick

1. Slides: for each channel 0, 1, 2, 3 whose slide flag is set, perform one slide step (4.4) and write
   AUDxPER.
2. Counter: decrement (8-bit). If it is not 0, the tick ends here.
3. Row: if any of the 4 words of the current row is 0xFFFF, song end (4.5). Otherwise play the row:
   counter = tempo; for channels 0, 1, 2, 3 in order: if the channel's entry has b0 != 0 and
   b1 != 0xFE, start a note (4.2), and then, if the same channel's entry in the NEXT row has b1 = 0xFE,
   set up a slide (4.4). Finally the row pointer moves to the next row (+8 bytes).

Since the counter starts at tempo, row 0 is played on tick number tempo (counting the first call as
tick 1); with tempo 1 on the first call.

### 4.2 Note start, channel c (exact write order)

Values:
- volume v = (b0 & 0x3F) + 1;
- sample s = b1 >> 4: address = sample address (1.3) + start offset of s; length = special length of s
  if non-zero, else the sample's length (1.3);
- period P = table(n) adjusted by octave (4.3);
- loop pair: if bit s of the loop word is set, (same address, same length); else (address of a 4-byte
  zero buffer owned by the player, length 1).
- The channel's slide flag is cleared.

Writes:
1. DMACON = 1 << c (channel off)
2. AUDxLC = address
3. AUDxLEN = length
4. AUDxPER = P
5. AUDxVOL = hw(v) (2)
6. a wait of nine raster lines (5.3)
7. DMACON = 0x8000 | (1 << c) (channel on)
8. nine raster lines again
9. AUDxLC = loop address
10. AUDxLEN = loop length

### 4.3 Periods

Table by note n (index n - 1):

| n | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 | 10 | 11 | 12 | 13 | 14 | 15 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| period | 204 | 216 | 229 | 242 | 257 | 272 | 288 | 305 | 323 | 343 | 363 | 384 | 0x666C | 0x7279 | 0x8088 |

(13..15 are what the reference reads past the 12 real entries: pairs of a following table of byte
values.) Note 0 reads a fixed word that lies in the player, not in the module: its value is in
`data/note0-period.bin` (2 bytes, big-endian); the lead decides about embedding (README).

Octave at a note start: o = 2 or 3: P = table >> 1; o = 0: P = (table << 1) & 0xFFFF; o = 1: P = table.

### 4.4 Slide (portamento)

Set-up, channel c, after its note start in row R, when row R+1's entry of channel c has b1 = 0xFE:
- search rows R+2, R+3, ..., R+36 (at most 35 rows) for the first entry of channel c whose b0 & 0x3F is
  non-zero (only the volume bits count; the entry's b1 does not matter; the search reads the end-mark
  rows and beyond like any other row). Not found: no slide (flag stays clear).
- found at row R+k: target T = table(n of that entry), adjusted with a DIFFERENT octave rule: o = 1 or
  3: unchanged; o = 2: >> 1; o = 0: << 1 (16-bit).
- N = k * tempo (ticks until that row).
- d = (T - P) as a 16-bit value, then taken as signed. base = d / N truncated toward zero, rem =
  |d - base * N|. sec = +1 if T > P compared as signed 16-bit numbers, else -1 (so d = 0 gives -1, and a
  target of 0x8000 or above, e.g. notes 13..15, counts as negative). acc = 0. Flag set.

Slide step (each tick, before the counter, while the flag is set):
P += base; acc += rem; if acc >= N (signed 16-bit): P += sec, acc -= N. All 16-bit. Write AUDxPER = P.

The flag is only cleared by the next note start on that channel (normally the target row, reached after
exactly N steps; the slide keeps running past the target if that row has no note start, e.g. its b1 is
0xFE). P is the period used by the next set-up.

### 4.5 Song end

When a row has a 0xFFFF word: the host is told "song end" (every time this happens). Then:
- length (1.4) > 2: the row pointer returns to offset 2 and that first row is played in the same tick
  (4.1 step 3, counter reset to tempo);
- length <= 2: nothing more in that tick; the counter stays 0, so the next row check comes 256 ticks
  later (it finds the same end mark again). Slides keep running. (Not observable in the logs: the host
  stops after the song end.)

The reference host stops a few ticks after the song end (1-4 ticks in the logs).

## 5. Timing

### 5.1 Clock model

Ticks are called every 70 940 cc. For a tick called at clock C keep a cycle count T starting at 0. A
write gets the stamp C + floor(T / 2) where T already includes the step that makes the write; a beam
read likewise. The song-end notification (4.5) re-bases T: at that point T is rounded down to even.
All costs are fixed (branch costs do not depend on the direction).

### 5.2 Tick ledger (cycles)

| step | cost |
|---|---|
| tick entry | 250 |
| slide check | 25; if any slide flag is set: + 18, then per channel 0..3: 29, plus 18 + slide step for a sliding channel; then + 16 |
| slide step | 101; + 44 if acc reached N; + 40 -> AUDxPER write; + 64 after it |
| counter | 29 (the tick ends here if the counter is not 0) |
| row end check | 16, then per word i = 0..3: 23 (i = 0) or 27 (i > 0); stops at the first 0xFFFF word |
| song end | 90, round T down to even, + 89; then length > 2: + 58 and play the row; else + 16, tick done |
| play row | 24 |
| per channel, entry test | 19 (channel 0) or 23 (1..3); b0 = 0: next channel |
| slide-marker test | 25; b1 = 0xFE: next channel |
| DMACON off | 20 -> write |
| note values | 36 + 320 (+ 16 for channels 1..3) + 8 if o is 2 or 3 + 8 if o is 0 + 149 + 16 if the special length is non-zero + 65 + 72 (looping sample) or 64 (not looping) |
| AUDxLC | 32 -> write (both words) |
| AUDxLEN | 24 -> write |
| AUDxPER | 24 -> write |
| AUDxVOL | 120 -> write |
| before wait | 80, then line wait (5.3) |
| DMACON on | 20 -> write, then line wait (5.3) |
| loop AUDxLC | 32 -> write |
| loop AUDxLEN | 24 -> write |
| next-row marker test | 25; marker present: + 18 + slide set-up (5.4) |
| after channel 3 | 44 (row pointer advance, end of tick) |

### 5.3 Nine-line wait

Cost 46, then nine times: { 16, read the line L0; then repeat { 16, read line L; 9 } until L != L0; then
10 }, then 44. "Read the line" = floor(stamp / 227) with the stamp of 5.1 (lines of 227 cc counted from
clock 0; frames of 313 lines, but every line boundary changes the value read, so only the boundary
matters). Each wait therefore ends 9 line boundaries later, at a time that depends on the beam phase.

### 5.4 Slide set-up cost

34; then for each searched row: 37, and if it is not a hit: + 17 (35 misses end the search).
- not found: + 24 + 4 + 9 (channel 3: 11) + 16.
- found: + 99; octave of the found entry: o = 1 or 3: + 0; o = 2: + 19 + 18; o = 0: + 19 + 8; then + 82;
  then + 4 + 9 (channel 3: 11) + 65; + 20 if sec = -1; + 195; + 20 if d - base * N < 0; + 52.

### 5.5 First tick

The first tick's clock depends on the start-up calls the reference makes (they write nothing). The host
calls them one after the other, starting at clock 100 of the run; each call's time is its cycle count
halved and rounded down, where an inner host service call also rounds down at that point:
- check: 388 + 29 * (zero words at the end of the file);
- companion load: 60 | 16 + 18 + 16 + 17 * (D + 1) + 28 + 21 * (F + 1) + 96 + X + 78 + 32 | 45, where
  D = length of the module's directory string as the host gives it (with its trailing `/`), F = length
  of the module's file name, X = 21 if the first character is `q` + 25 if the second is `p` + 25 if the
  third is `a` (lower case costs a second comparison); each "|" is a rounding point;
- initialisation: 36 | 574 + 23 * L | 394 + 27 * (zero length words) + 51 * (non-zero length words) +
  86 * (lengths <= 50) + 88 * (lengths > 50) | 16, where L = number of longs scanned in 1.4 (including
  the 0xFFFFFFFF one);
- volume: 236; balance: 236; sound init: 1338.
The first tick is called 70 940 cc after the last of these returns. Example: a 202-byte module with no
padding, name `QPA.basic` in a 90-character directory: first tick at clock 75 381.

## 6. Data words not in the module

- Note 0's period word (4.3): `data/note0-period.bin`.
- The 4-byte zero buffer used as the loop of non-looping samples: any 4 zero bytes in chip memory.
