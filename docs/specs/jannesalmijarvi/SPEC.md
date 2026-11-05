# Janne Salmijarvi Optimizer (`JS.*`) replayer: behaviour spec

*Sanitised for publication: behaviour only.*

Scope: the "Janne Salmijarvi Optimizer" module format (prefix `JS.`, magic `JS92`), as played by the
UADE/Eagleplayer player of that name (registered in the host as `Janne Salmijarvi Optimizer`;
the player file is `Janne_Salmijarvi_Optimizer`). The replay routine comes from the music disk
"A Taste Of U4IA" (Megawatts, 1992). It is a ProTracker 2.x style replayer with a different
pattern-cell encoding, a few behaviour differences from ProTracker (listed in section 9), and an
interrupt that runs four times per tick and spreads the note start over those four calls.

This text describes behaviour observed in the reference (the 68k player run through the host with
DELI_WRITE_LOG, DELI_DMA_LOG and DELI_TRACE). It contains no code of the original. Times in section
8 are 68000 CPU cycles as the reference machine charges them; the write clock in colour clocks
("cc") is the interrupt entry clock plus floor(cycles / 2).

Status (checked by a private model that follows the rules below, against the reference):

| item | result |
|---|---|
| 7 synthetic modules (no real module exists in the corpus), 10 210 Paula and CIA writes | value, order and colour clock of every write equal the reference (0 differences) |
| DMA log of the invert-loop module (54 526 audio fetches from module memory) | equal to the model's sample memory |
| replay code paths | every path of the tick code was executed by at least one reference run |

## 1. File format

All numbers big-endian. Offsets are from the start of the file.

| offset | size | content |
|---|---|---|
| 0 | 20 | song title (shown by the host as the song name) |
| 20 | 31 x 30 | sample headers, see below |
| 950 | 1 | song length L (number of positions played) |
| 951 | 1 | pattern-count scan length minus one (see 1.2); 0x7F in normal files |
| 952 | 128 | position list: pattern number per position |
| 1080 | 4 | magic `JS92` |
| 1084 | P x 1024 | patterns (P from 1.2) |
| 1084 + P x 1024 | ... | sample data, samples 1..31 back to back, each `2 x length` bytes |

Sample header (30 bytes, sample n = 1..31 at offset 20 + 30 x (n - 1)):

| offset | size | content |
|---|---|---|
| 0 | 22 | name |
| 22 | 2 | length in words |
| 24 | 1 | finetune, 0..15 (used directly as the period-table row, 8..15 = ProTracker's -8..-1) |
| 25 | 1 | volume |
| 26 | 2 | loop start in words |
| 28 | 2 | loop length in words (1 = no loop) |

Pattern: 64 rows x 4 voices x 4 bytes; the cell of row r, voice v is at
`1084 + pattern x 1024 + r x 16 + v x 4`. Cell bytes:

| byte | content |
|---|---|
| 0 | note: 0 = none, otherwise `2 x (k + 1)` for note index k = 0..35 (C-1 .. B-3); it is a byte offset into the period table of section 2.1 |
| 1 | sample number 0..31 (0 = keep) |
| 2 | command 0..15 (a whole byte, not a nibble) |
| 3 | command parameter |

The 16-bit "command word" used below is `byte 2 x 256 + byte 3`.

### 1.1 Detection

The file is longer than 2112 bytes and the 4 bytes at offset 1080 are `JS92`. Nothing else is
checked. (The prefix `JS.` is the usual file name prefix.)

### 1.2 Derived values

- Pattern count P: scan the bytes at 952, 953, ... for `(byte 951) + 1` entries (128 for 0x7F),
  treating each byte as signed; P = 1 + the largest value seen (starting from 0; negative bytes
  never count). The sample data starts at `1084 + 1024 x P`.
- Sample start of sample n: the sample data start plus `2 x` the sum of the lengths of samples
  1..n-1.
- The module is rejected ("too short") when `1084 + 1024 x P + 2 x (sum of the 31 lengths)` is
  larger than the file.
- Before play the first 16-bit word of the sample data (the first two bytes of sample 1) is set
  to 0. This word is the "empty loop" used by samples without a loop (section 4.2); it changes
  what sample 1 plays.
- Host information: title = the 20 title bytes; number of samples = samples with a non-zero
  length; song length = byte 950; patterns = P. There is one subsong.

## 2. Tables

### 2.1 Period table

16 rows (finetune 0..15) of 36 periods (note index 0..35). It is the standard ProTracker 2.x
finetune period table (row 0 starts 856, 808, 762; row 8 starts 907, 856, 808) with one
difference: row 9, note index 23 is 230 instead of 238. `data/periods.bin` holds the 576 values
(16-bit big-endian, row-major) as the reference player has them.

Lookups use a byte offset `o` from a point one word before row 0: the value is the period at
row-major index `o / 2 - 1`; for `o / 2 = 0` it is the special value 0x4080. So "period of note
byte N with finetune f" is the entry at offset `N + 72 x f`, i.e. row f, note index `N / 2 - 1`
for valid notes. An offset that is a multiple of 72 lands on the special word (f = 0) or on the
last entry of row f - 1; this happens only for arpeggio before any note (section 5.1).

### 2.2 Other tables

- Vibrato/tremolo sine (32 values): 0, 24, 49, 74, 97, 120, 141, 161, 180, 197, 212, 224, 235,
  244, 250, 253, 255, 253, 250, 244, 235, 224, 212, 197, 180, 161, 141, 120, 97, 74, 49, 24.
- Invert-loop speeds (16 values): 0, 5, 6, 7, 8, 10, 11, 13, 16, 19, 22, 26, 32, 43, 64, 128.
- Arpeggio step for tick counter t (0..31): t mod 3.

## 3. Timing frame

- The player interrupt runs four times per tick. Its period in colour clocks is
  `(floor(1773375 / (4 x T)) + 1) x 5` for tempo T; 1773375 = 125 x 14187 (14187 is the host's
  50 Hz CIA value). Initial tempo 125: 17735 cc per interrupt, 70940 cc per tick.
- A tempo command (section 6, command F with parameter >= 0x20) changes the period one interval
  late: the next interrupt still comes one old period after the entry of the interrupt in which
  the command ran, and the new period applies from the interval after that one. Observed: 0x40 -> 34640, 0x9A -> 14395, 0xFF -> 8695,
  0x7D -> 17735.
- Each interrupt has a phase 0..3 (starting at 0, then 1, 2, 3, 0, ...); phase 0 runs the tick
  (sections 4 to 7), phase 1 turns DMA on, phase 2 sets the loop registers, phase 3 does nothing
  (section 7). The phase counter advances after every interrupt and is never reset.

## 4. State

### 4.1 Global (initial values)

speed 6; tick counter 0; position 0; row offset 0 (row x 16); break row 0; jump flag clear;
loop-jump flag clear; slide mask 0 (not 0xFF: see 6.1); pattern delay 0 and delay counter 0;
note mask 0 (voices to switch on in phase 1); retrigger mask 0 (voices retriggered; phase 1/2);
phase 0.

### 4.2 Per voice (all start at 0 except the DMA bit)

current cell (4 bytes); sample start; length; loop start; loop length; period; finetune; volume;
DMA bit (1, 2, 4, 8 for voices 0..3); tone-porta direction, speed and target period; vibrato
command byte (high nibble speed, low nibble depth) and position; tremolo command byte and
position; wave control byte (low nibble vibrato, high nibble tremolo; bit 2 / bit 6 = "do not
reset the position on a new note"); glissando/invert byte (low nibble glissando on/off, high
nibble invert-loop speed); sample-offset memory; loop row; loop counter; invert-loop accumulator;
invert-loop pointer; last note byte.

Sample selection (cell byte 1 = n, not 0): sample start = start of sample n; length = header
length; finetune and volume from the header; loop length from the header. If the loop length is
1 the loop start is the empty-loop word (the start of the sample data, section 1.2); otherwise the
loop start is `sample start + 2 x header loop start`. The invert-loop pointer is set to the loop
start. Unlike ProTracker the length is not shortened to `loop start + loop length`.

## 5. Phase 0: the tick

Add 1 to the tick counter (8 bits).

- Counter < speed: **effect tick**: run the effect pass (5.1) for voices 0..3, then while the
  jump flag is set, go to the next position (5.4).
- Otherwise the counter becomes 0 and:
  - if the delay counter is non-zero: **delayed row**: run the effect pass with counter 0 (the
    same per-voice rules as an effect tick, so commands that act "on counter 0" act again), then
    advance the row (5.4);
  - else **row tick**: clear the note mask, read and play the row (5.2) for voices 0..3, then
    advance the row (5.4).

The first row is played on the sixth interrupt-0 call (counter 1..5 are effect ticks first).

### 5.1 Effect pass (one voice)

1. Invert-loop step (5.5).
2. If the command word is 0: write the current period to AUDxPER. Done.
3. Otherwise by command (section 6, "tick" column). Commands 8, 9, B, C, D, F do nothing on
   effect ticks: no register is written (ProTracker would rewrite the period).

### 5.2 Row read (one voice)

The 4 cell bytes are read into the voice (the pattern pointer moves on by 4 per voice).

1. All four bytes 0: nothing at all happens for this voice in this row (no write, no invert-loop
   step, no command). This differs from ProTracker.
2. Sample number non-zero: select the sample (4.2).
3. Note byte 0: run the row commands (5.3). Done.
4. Note byte non-zero:
   - command E with parameter 5x: set finetune = x, then start the note (step 5).
   - command 3 or 5: tone-porta setup: target = period of the note byte with the voice's
     finetune; direction 0 (target >= period, compared as signed 16-bit) or 1 (target < period);
     if target = period the target becomes 0. Then run the row commands (5.3), then output the
     volume (8.4). The note is not started and the last note byte is not changed.
   - command 9: run the row commands (5.3) first, then start the note (step 5), which runs
     the row commands again (so the sample offset is applied twice, as in ProTracker).
   - anything else: start the note.
5. Note start: last note byte = note byte; period = period of the note byte with the voice's
   finetune. If the command is E with parameter Dx (note delay): run the row commands (5.3) only
   (the delay command starts the note later). Otherwise:
   write the voice's DMA bit to DMACON (voice off); if wave-control bit 2 is clear set the
   vibrato position to 0; if bit 6 is clear set the tremolo position to 0; write AUDxLC = sample
   start, AUDxLEN = length, AUDxPER = period; output the volume (8.4); add the voice's DMA bit to
   the note mask; then run the row commands (5.3).

### 5.3 Row commands (one voice)

1. Invert-loop step (5.5).
2. By command (section 6, "row" column). Commands 0..8 and A write the current period to AUDxPER
   (also command 3/5, also a zero command word).

### 5.4 Row advance and positions

Advance: row offset += 16. If the pattern delay is non-zero, the delay counter takes it and the
delay becomes 0. If the delay counter is non-zero, subtract 1, and if still non-zero, row offset
-= 16 (the row repeats). If the loop-jump flag is set: clear it, row offset = break row x 16,
break row = 0. If row offset >= 1024: go to the next position. Then while the jump flag is set,
go to the next position.

Next position: row offset = break row x 16 (16-bit); break row = 0; jump flag clear;
position = (position + 1) and 0x7F; if position >= L (unsigned), position = 0 and the song-end
notification is sent.

The pattern of position p is byte `952 + p`; a row is read at `pattern x 1024 + row offset`
(a break row above 63 reads beyond the pattern into the following ones; see 6.1 command D).

### 5.5 Invert-loop step

If the voice's invert-loop speed s (high nibble of the glissando/invert byte) is 0 nothing
happens. Otherwise accumulator = (accumulator + speed table[s]) and 0xFF; if bit 7 is now clear
nothing more happens. Else: accumulator = 0; pointer += 1; if pointer >= loop start +
2 x loop length the pointer becomes the loop start; the sample byte at the pointer becomes
255 - byte (in sample memory, permanently).

## 6. Commands

"Counter" is the tick counter (0 on row ticks and delayed rows). `x` / `y` are the high / low
parameter nibbles.

| cmd | row (counter 0) | tick (counter > 0, and delayed rows) |
|---|---|---|
| 0 | period write | if command word non-zero: arpeggio (6.2) |
| 1 | period write | slide up (6.1) |
| 2 | period write | slide down |
| 3 | period write (after the setup of 5.2) | tone porta (6.3) |
| 4 | period write | vibrato (6.4) |
| 5 | period write | tone porta continue, then volume slide |
| 6 | period write | vibrato continue, then volume slide |
| 7 | period write | tremolo (6.5) |
| 8 | period write | nothing |
| 9 | sample offset (6.1) | nothing |
| A | period write | volume slide (6.6) |
| B | position jump | nothing |
| C | set volume | nothing |
| D | pattern break | nothing |
| E | extended (6.7) | extended |
| F | speed / tempo | nothing |

### 6.1 Simple commands

- Slide up (1): d = parameter AND slide mask; slide mask = 0xFF; period -= d; if period (signed)
  is not above 113, period = 113. Write AUDxPER. The slide mask starts at 0, so the very first
  slide of the whole song (on any voice) moves by 0. Fine slides set it to 15 first (6.7).
- Slide down (2): the same with period += d, limit: if period (signed) is not below 856,
  period = 856.
- Sample offset (9): if the parameter is non-zero it is remembered; o = remembered value x 128
  (words); if o < length (signed 16-bit compare): length -= o, sample start += 2 x o; else
  length = 1. No write.
- Position jump (B): position = parameter - 1 (8 bits); break row = 0; jump flag set; and the
  song-end notification is sent (every B command, at the row). The next-position step then moves
  to `parameter`, or wraps to 0 with a second notification if `parameter >= L`.
- Set volume (C): volume = parameter, at most 64 (unsigned); output the volume.
- Pattern break (D): break row = parameter as a plain number (no BCD decoding, unlike
  ProTracker: D30 goes to row 48); jump flag set.
- Speed/tempo (F): parameter 0: nothing (no stop). Below 0x20: speed = parameter, counter = 0.
  From 0x20: tempo = parameter (section 3); speed unchanged.

### 6.2 Arpeggio (0, on ticks, command word non-zero)

Step = counter mod 3. Step 0: value = current period. Step 1 / 2: n = x / y; offset =
last note byte + 2 x n, at most 72; value = period table at offset `offset + 72 x finetune`
(section 2.1). Write the value to AUDxPER (the stored period is unchanged).

### 6.3 Tone porta (3; 5 uses the same body without the parameter part)

Command 3 only: if the parameter is non-zero, speed = parameter and the parameter byte of the
voice's cell is cleared (so later ticks of the row see 0). Then: if the target is 0 nothing
happens (no write). Otherwise direction 0: period += speed; if target (signed) > period it stays,
else period = target and target = 0. Direction 1: period -= speed; if target < period it stays,
else period = target and target = 0. Value = period; if glissando is on (low nibble non-zero):
find in row `finetune` of the period table the first entry e (index 0..35) with
value >= entry (unsigned); if none, the index-35 entry; value = that entry. Write the value to
AUDxPER.

### 6.4 Vibrato (4; 6 uses the body without the parameter part)

Command 4 only: if the parameter is non-zero, a non-zero low nibble replaces the depth and a
non-zero high nibble replaces the speed. Then i = (position >> 2) and 31. Waveform (wave control
and 3): 0: v = sine[i]; 1: v = (i x 8) and 0xFF, replaced by 255 - that when the vibrato
position has bit 7 set; 2 or 3: v = 255. a = (v x depth) >> 7. Value = period - a if the
position's bit 7 is set, else period + a. Write AUDxPER. Position = (position + speed x 4) and
0xFF.

### 6.5 Tremolo (7)

First write the current period to AUDxPER. Parameter handling as vibrato (into the tremolo
byte). i = (tremolo position >> 2) and 31. Waveform (wave control >> 4, and 3): 0: sine[i];
1: (i x 8) and 0xFF, replaced by 255 - that when the **vibrato** position has bit 7 set (as in
ProTracker); 2, 3: 255. a = (v x depth) >> 6. w = volume - a if the tremolo position's bit 7 is
set, else volume + a; below 0 -> 0, above 64 -> 64. Output w (8.4; the stored volume is
unchanged). Tremolo position += speed x 4 (8 bits).

### 6.6 Volume slide (A, and the second half of 5 and 6)

Command A first writes the current period to AUDxPER. If x is non-zero: volume = (volume + x)
and 0xFF; if (volume - 64) as a signed byte is not negative, volume = 64. Else: volume =
(volume - y) and 0xFF; if bit 7 is set, volume = 0. Output the volume.

### 6.7 Extended commands (E, sub-command x, value y)

| E x | behaviour |
|---|---|
| 0 | every time it runs (row and ticks): CIA-A port A = (port A and 0xFD), then port A = port A or the whole parameter byte (two CIA writes; parameter 0x00 = filter on, 0x02 = off) |
| 1 | counter 0 only: slide mask = 15, then slide up (6.1) with the parameter |
| 2 | counter 0 only: slide mask = 15, then slide down |
| 3 | glissando nibble = y (every time) |
| 4 | vibrato waveform nibble = y (every time) |
| 5 | finetune = y (every time; on a row with a note it is set before the note's period, 5.2) |
| 6 | counter 0 only: y = 0: loop row = current row offset / 16. Else if the loop counter is 0: counter = y and jump; else counter -= 1 and jump unless it became 0. Jump = break row = loop row and the loop-jump flag set |
| 7 | tremolo waveform nibble = y (every time) |
| 8 | nothing |
| 9 | retrigger: y = 0: nothing. On counter 0 with a note byte: nothing. Otherwise retrigger (6.8) when the counter is a positive multiple of y (counter 0 never retriggers; ProTracker does when the cell has no note) |
| A | counter 0 only: volume up by y (as 6.6, clamp at 64) |
| B | counter 0 only: volume down by y (clamp at 0) |
| C | when counter = y: volume = 0, output 0 |
| D | when counter = y and the cell has a note byte: retrigger (6.8). D0 on the row retriggers at once |
| E | counter 0 only, and only if the delay counter is 0: pattern delay = y + 1 |
| F | counter 0 only: invert speed nibble = y; if y is non-zero, run one invert-loop step at once (5.5) |

Pattern loop and pattern delay interact as in ProTracker: during the delayed repeats of a row,
E6 runs again (counter 0), so E6y and EEy on the same row loop forever.

### 6.8 Retrigger

Write the voice's DMA bit to DMACON (voice off), AUDxLC = sample start, AUDxLEN = length,
AUDxPER = period, output the volume, and add the voice's bit to the retrigger mask (not to the
note mask).

## 7. Phases 1, 2, 3

- Phase 1: if the retrigger mask is non-zero: mask |= 0x8000 and write the mask to DMACON.
  Else if the note mask is non-zero: write `note mask | 0x8000` to DMACON. Else nothing.
- Phase 2: if the retrigger mask is non-zero: for voices 0..3 whose bit is set, write AUDxLC =
  loop start and AUDxLEN = loop length; then the retrigger mask = 0. Else if the note mask is
  non-zero: for all four voices (also voices without a new note, and voices never used, which
  write 0 and 0) write AUDxLC = loop start and AUDxLEN = loop length; then the note mask = 0.
- Phase 3: nothing.

While the retrigger mask is set, the note mask is not served: voices that started a note in the
same tick as a retrigger stay off until a later tick's phase 1 (the row tick clears the note
mask before reading the next row).

## 8. Register writes: values, order and timing

### 8.1 Values

- AUDxLC: sample start / loop start as absolute addresses in the module image (the empty-loop
  word for loop length 1); never-used voices write 0. Written as one long write: both halves at
  the same clock, high half first.
- AUDxVOL: see 8.4. AUDxPER, AUDxLEN, DMACON: as in sections 5 to 7.
- CIA-A port A writes come from E0 only (and one at init).

### 8.2 Clock rule

Every interrupt starts at its entry clock E (section 3). The tick code charges 68000 cycles
(static figures, see README); a write happens at `E + floor(C / 2)` colour clocks where C is the
cycle count from the entry up to and including the writing step. The steps below give C as sums;
"-> W" marks a write after the cycles before it.

Voice-dependent helper costs (voice 0, 1, 2, 3):

| name | cycles |
|---|---|
| PB (period bookkeeping after a period write) | 79, 110, 141, 149 |
| VO (volume output: from its start to the AUDxVOL write) | 192, 215, 238, 228 |
| VA (after the AUDxVOL write) | 113, 144, 175, 183 |
| SB (sample bookkeeping after AUDxPER in a note start or retrigger) | 133, 164, 195, 203 |
| PL (effect-pass lead-in before the voice) | 38, 34, 34, 16 |

Invert-loop step cost F: speed 0: 109; accumulator does not overflow: 166; step without
pointer wrap: 311; step with wrap: 327.

### 8.3 Step costs

Interrupt entry: 281. Phase 3: nothing written. Phases 1 and 2: +42, then

- retrigger mask non-zero: phase 1: +81 -> W DMACON. Phase 2: +76; then per voice 0..3: bit set:
  +51 (voice 0) or +55 (voices 1..3) -> W LC, +20 -> W LEN; bit clear: +27.
- else +23; note mask zero: nothing; phase 1: +73 -> W DMACON; phase 2: +76, then per voice
  +28 -> W LC, +20 -> W LEN.

Phase 0: +18, then effect tick +69, delayed row +108, row tick +90.

**Effect pass** (effect ticks and delayed rows), per voice in order: + PL, + F, + 41, then

- command word 0: +20 -> W PER, then +42 + PB + 28.
- otherwise +50, then by command:
  - 8, 9, B, C, D, F: +16.
  - 0 (arpeggio): +49; step 0: +34 -> W PER; step 1: +59 +85 (+4 if the offset was limited to
    72) +30 -> W PER; step 2: +49 +85 (+4) +30 -> W PER. Then +34 + PB + 28.
  - 1, 2: +91 (+16 if limited) +20 -> W PER, then +42 + PB + 28.
  - 3: +23 (+28 if the parameter is non-zero) then tone-porta body.
  - tone-porta body: +23; target 0: +16 (no write). Else +39, then direction 0: +39
    (+46 if it reached the target), direction 1: +51 (+36 if reached); then +43; glissando off:
    +0; on: +58 plus, for a match at index i, (i + 1) x 25 + i x 23 (no match at all:
    36 x 25 + 36 x 23 + 4), then +14; then +12 -> W PER, then +34 + PB + 28.
  - 4: +23; parameter non-zero: +31 (+12 if low nibble non-zero) +31 (+12 if high nibble
    non-zero) +12. Then vibrato body.
  - vibrato body: +65; waveform 0: +14; waveform 1: +31 +23 then +14 (position bit 7 clear) or
    +22 (set); waveform 2/3: +31 +18. Then +129, then +4 (position bit 7 set) or +14, +12 -> W PER,
    then +18 + PB + 62.
  - 5: +18, tone-porta body (including its post-write cost, or the +16 without target), +10,
    volume slide.
  - 6: +18, vibrato body (including its post-write cost), +10, volume slide.
  - 7: +20 -> W PER, +42 + PB + 12, then +23; parameter non-zero: +31 (+12) +31 (+12) +12 as for
    vibrato; then +79; waveform 0: +14; 1: +31 +23 then +14 / +22 (vibrato position bit 7 clear /
    set); 2/3: +31 +18; then +131; +4 (tremolo position bit 7 set) or +14; +11 (+4 if the result
    was negative, then clamped to 0); then always +19, or +27 instead if above 64; then volume output: + VO -> W VOL, + VA, +62.
  - A: +20 -> W PER, +42 + PB + 12, then volume slide.
  - E: +56, then extended command (below).

**Volume slide**: +45; up (x non-zero): +43 (+16 if limited to 64) +20 + VO -> W VOL, + VA + 16.
Down: +24, then +27 (+16 if limited to 0) +20 + VO -> W VOL, + VA + 16.

**Extended command** (after its +56):

- E0: +36 -> W CIA (and 0xFD), +20 -> W CIA (or parameter), +16.
- E1: +23; counter non-zero: +16; else +16, then slide up (+91 ...) as command 1. E2: +27 (instead
  of E1's +23); counter non-zero: +16; else +16, then slide down as command 2.
- E3 +72; E4 +72; E5 +48; E7 +86; E8 +16.
- E6: +27; counter non-zero: +16. Else +31; y = 0: +54. Else +23; loop counter 0: +22 +52; counter
  becomes 0: +27 +16; otherwise +27 +52.
- E9: +35; y = 0: +16. Else +27; counter 0: +23, and +16 (stop) if the cell has a note. Then per
  subtraction of y from the counter: +15; reaching exactly 0: retrigger; going below 0: +27 (stop);
  otherwise +11 and subtract again.
- EA: +23; counter non-zero: +16; else +34, then the "up" part of the volume slide from its +43.
- EB: +23; counter non-zero: +16; else +34, then the "down" part from its +27.
- EC: +47; counter differs: +16; else +32 + VO -> W VOL (0), + VA + 28.
- ED: +47; counter differs: +16; else +23; no note: +16; note: +10, retrigger.
- EE: +23; counter non-zero: +16; else +47; delay counter busy: +16; else +32.
- EF: +23; counter non-zero: +16; else +85; y = 0: +16; else + F (the step's own cost).

**Retrigger**: +24 -> W DMACON (voice bit), +24 -> W LC, +20 -> W LEN, +20 -> W PER, + SB,
+42 + PB + 12, +20 + VO -> W VOL, + VA + 44.

**Row tick**: +188 before voice 0, +34 before each of voices 1..3. Per voice:

- +57; all four cell bytes 0: +16, done.
- +27; sample number non-zero: +209, then +66 (loop length 1) or +102.
- +23; note byte 0: row commands. Otherwise +45, then
  - command E5x: +66 +10, note start.
  - else +31; command 3: tone-porta setup. Else +19; command 5: tone-porta setup. Else +19;
    command 9: +18, row commands, +10, note start; other commands: note start.
- Tone-porta setup: +177; then +32 (target = period), +27 (target above) or +43 (below); +18,
  row commands, +20 + VO -> W VOL, + VA + 16.
- Note start: +141; command EDx: row commands only. Otherwise +24 -> W DMACON (voice bit), +43
  (vibrato position reset) or +27, +43 (tremolo position reset) or +27, +24 -> W LC, +20 -> W LEN,
  +20 -> W PER, + SB, +42 + PB + 12, +20 + VO -> W VOL, + VA + 38, row commands.
- Row commands: +18 + F + 50, then by command:
  - 0..8, A: +20 -> W PER, +42 + PB + 28.
  - 9: +27 (+12 if the parameter is non-zero) +55, then +60 (offset fits) or +32.
  - B: +225 (includes the host's song-end call, counted as 15).
  - C: +35 (+4 if above 64) +20 + VO -> W VOL, + VA + 16.
  - D: +52.
  - E: +56 and the extended command (with counter 0).
  - F: +27; 0: +16; below 0x20: +63; from 0x20: +345 (includes the division and the host's
    timer call, counted as 15).

### 8.4 Volume output

The value written to AUDxVOL is `((v and 0x7F) x M) >> 6` (16-bit), where M is the host volume
for the voice (voices 0 and 3 left, 1 and 2 right; `balance x master / 64`, 64 by default, so the
value is `v and 0x7F`). A muted voice writes 0. The player also remembers the last value per voice
so that a host volume change can rewrite all four (the host does this twice before init in the
logs: eight writes of 0, section "Start" in LOGS.md).

### 8.5 Start

Init (before the first interrupt): CIA-A port A |= 0x02 (written), AUDxVOL = 0 for voices 0..3,
DMACON = 0x000F (all voices off); speed 6, tempo 125. Nothing else is written until the first
interrupt.

## 9. Differences from ProTracker 2.x (summary)

- Cell encoding (note as table offset, full command byte, full sample byte); magic `JS92`;
  pattern count from a signed scan of `(byte 951) + 1` entries.
- Sample length is never shortened by the loop; loop length 1 uses the zeroed first word of the
  sample data.
- A completely empty cell does nothing at all (no period write, no invert-loop step).
- Commands 8, 9, B, C, D, F do not rewrite the period on effect ticks.
- The slide mask starts at 0 (first slide of the song moves by 0).
- D takes its parameter as a plain row number; F00 does not stop; every B sends the song-end
  notification.
- E9 never retriggers on counter 0.
- DMA on and loop registers are written in two later interrupt calls (phases 1 and 2), the loop
  registers of all four voices at once.
- The period table differs in one entry (2.1).
