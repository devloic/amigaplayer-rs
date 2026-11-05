# Speedy A1 System (Michael Winterberg, 1988-89) - behaviour specification

*Sanitised for publication: behaviour only.*

Format: "Speedy A1 System" modules (UADE player SpeedyA1System, Eagleplayer name "Speedy A1 System",
prefix `sas.`, in the wild also seen with the extension `.sas`). A four-voice Paula tracker: a 512-entry
order list of four track numbers per position, 16-row tracks of 3-byte rows, 32 sample slots, per-note
attack/release volume envelopes, a period slide effect, 8SVX-style samples that the player never reads
itself (only their addresses and lengths go to Paula). One subsong per file, no title stored.

This player is **not** the same player as "Speedy System" (prefix `ss.`); it has a different file layout,
different tables and different tick costs. See section 9 for what is shared and what differs. The
Speedy System specification is not needed to implement this one.

All timing below is in 68000 cycles counted from the start of the tick (the moment the CIA timer
interrupt enters the player) unless stated as colour clocks (cc). A register write whose cycle offset
from the tick start is `c` is stamped at `tick_start + floor(c/2)` cc. The cycle numbers were measured
with the reference 68k player run through the host with DELI_WRITE_LOG (the host charges a fixed static
cycle count per instruction), so they reproduce the host logs exactly; they are not claims about real
hardware. A write to a 32-bit AUDxLC appears in the log as two word writes with the same stamp, high
word at the register, low word at register+2.

## 1. Module file

All multi-byte values are big endian. Offsets are from the start of the file.

| offset | size | content |
|---|---|---|
| 0x00 | 32 bit | speed S (see 3.1); must be below 65536 for detection |
| 0x04 | 32 bit | start position (first position played, and the loop-back position) |
| 0x08 | 32 bit | end position: the song ends when the position counter reaches this value after row 15 |
| 0x0C | 32 bit | 0x00000200 (512, the order list capacity) |
| 0x10 | 0x1000 | order list: 512 positions x 4 voices x 16-bit track number (position p, voice v at 0x10 + 8p + 2v) |
| 0x1010 | 48 x (Tmax+1) | tracks, 48 bytes each (16 rows x 3 bytes) |
| TB | 0x280 | five sample tables of 32 32-bit values each (TB is found as in 1.1) |
| TB+0x280 | ... | sample objects (8SVX FORM data), starting with the object of the first sample |

An order entry of 0xFFFF means "end of song" (4.2); a track number is otherwise below 256 (the detection
rejects larger ones before the first 0xFFFF).

### 1.1 Detection

A file is accepted when all of these hold:

1. its first 16-bit word is zero;
2. the 32-bit value at 0x0C is exactly 0x200;
3. scanning the order list word by word from 0x10 for up to 2048 words, stop at the first word equal to
   0xFFFF; every earlier word must have a zero high byte. Tmax is the largest word seen before the stop
   (all earlier words counted, ties included);
4. starting at 0x1010 + 48 x (Tmax+1), skip whole 32-bit words that are zero; the first non-zero 32-bit
   word found there is the first entry of the first sample table: its position is TB;
5. the four bytes at TB + 0x280 are the ASCII text `FORM`.

Consequence of 4: zero padding between the last track and the sample tables is allowed, but the first
entry of the first table (the object offset of sample slot 0) must be non-zero (a real module has 0x280
there). Tracks with numbers above Tmax must not exist (they would be taken for the table). No file
length check is made by the player. The module does not contain the text `SPEEDY-SYSTEM` and no
0x1190 marker; it cannot be confused with a Speedy System file (whose first word is not zero).

### 1.2 Sample tables

At TB there are five tables, each 32 big-endian 32-bit values, in this order:

| table | name | meaning |
|---|---|---|
| 0 | object offset | offset of the sample's 8SVX object, relative to TB. If the high byte is non-zero the slot is "empty" (seen: 0xFFFCBE40); the player only uses this for counting samples and for a load-time scan, not at playback |
| 1 | object length | byte length of the whole object including its IFF header |
| 2 | data offset | offset (relative to TB) of the first sample byte to play; in the one real module it equals object offset + 0x68 (an IFF header of 0x68 bytes: FORM, VHDR, NAME, ANNO, BODY chunk headers) |
| 3 | first part | byte offset, from the data start, at which the loop starts (the part played before the loop); Paula's first pass covers table-3 + table-4 bytes, the loop is the last table-4 bytes of it |
| 4 | loop length | byte length of the repeated part; 0 means the sample has no loop |

The player never looks at sample bytes. At playback it only needs tables 1 to 4 (and, for the load-time
scan, table 0). Addresses given to Paula are `load address of the module + TB + value`, 32 bit wrap. A
note may select any of the 32 slots; empty or inconsistent slots are not rejected (the table values are
used as they are).

### 1.3 Tracks and rows

A track is 16 rows of 3 bytes `[b0, b1, b2]`. A voice's track at position p is the track number in the
order list at (p, voice). There is no run-length coding; the same track may be used any number of times.
The meaning of a row is decided by b0:

* `0xFF`: end of song (4.2).
* `0x50 .. 0xFE`: command row (4.4): stores b0, b1, b2 for the voice.
* `0x00 .. 0x4F`: note row if the period table (4.3) gives a non-zero period for b0 (b0 = 16 x octave
  index + note number with note number 1..13), else the row is ignored. Octave index 0..4. Note
  number 0, 14 and 15 give period 0, so they are ignored; b0 = 0 is the normal "empty row".

For a note row, b1 is the envelope byte (0 = no envelope, see 4.3) and b2 the sample slot; a note whose
b2 is 0x20 or more is ignored (also if its period is valid).

### 1.4 Song information

* Subsongs: 1. Title: none stored.
* Number of samples (as reported by the player): the number of sample slots whose table-0 entry has a
  zero high byte.
* Number of positions (as reported): the number of complete 4-word groups in the order list before the
  first 0xFFFF word.
* Song length: not stored. The song plays positions start .. end-1 (each 16 rows, speed+1 ticks per row,
  at 50 Hz: 70940 cc per tick) and is cut short by the first order entry 0xFFFF or row byte 0xFF
  reached (4.2). The real module has start 0 and end 0 (so the position test never fires) and ends by an
  order entry 0xFFFF at position 482 after 462.7 s. Compute the exact length by running the replay.

## 2. Timing

One CIA timer interrupt of 70940 cc period (50.0 Hz PAL) drives the player. Each interrupt is one tick.
Tick k starts at `E0 + 70940*k` cc, E0 from section 8. The tick routine always completes within the
tick (the longest tick, four note starts with beam waits, stays below about 40000 cycles of the 141880 available).

## 3. Replay state

Per voice: state (idle, attack, release), current volume, current period, volume timer (16 bit),
attack value, release value, sample index of the running note (or none), the stored command byte and its
two argument bytes (16-bit values). Global: speed counter, position, row, DMA mask for the tick.

Initial state: everything zero; position = start position; row = 0; speed counter = 0 (so the first tick
is a full tick).

### 3.1 Tick types

* If the speed counter is 0 this is a **full tick**: the counter is set to the module's speed value S,
  the row of the current position is processed for voices 0, 1, 2, 3 in order (section 4), the DMACON
  enable write is made (4.5), then all four voices are updated (section 5), then the row counter is
  advanced (3.2).
* Otherwise it is a **light tick**: the counter is decremented and only the voice update (section 5) runs.

So a new row starts every S+1 ticks.

### 3.2 Row and position advance

At the end of a full tick the row counter is incremented. When it reaches 16 it is set to 0 and the
position is incremented; if the position then equals the end position field, the song-end report is made
and the position becomes the start position (the stored commands of all four voices are cleared, 4.2).
(A song-end report found inside row processing, 4.2, sets the row counter to -1 beforehand, so that the
increment brings it to 0 and the start position is played from its first row without a position step.)

## 4. Row processing (full ticks), voices in order

For voice v with order entry `t` at the current position:

### 4.1 Selecting the row

If the order entry is 0xFFFF, or the row byte b0 is 0xFF, the song ends (4.2). Otherwise rows are
handled as listed in 1.3: command rows (4.4), note rows (4.3), ignored rows (nothing happens).

### 4.2 Song end inside a row

The host is told that the song ended (the song-end report: it costs time, see 6.4). Position becomes the
start position, the row counter becomes -1 (so that it is 0 after this tick's increment), the stored
command bytes of all four voices are set to zero, and the remaining voices of this tick's row are NOT
processed (voices before it already were). The rest of the tick (4.5, section 5) runs normally. The speed
counter is not reset: the start position's first row is played S+1 ticks after this tick, as for any
row. The song then continues, looping.

### 4.3 Note start (accepted note on voice v)

An accepted note is a row with b0 < 0x50, non-zero period P (table below) and b2 < 0x20. Let i = b2,
and let T1..T4 mean entry i of sample tables 1..4 and `A2 = load address + TB + table-2 entry`.

Period P: octave index = b0 >> 4 (0..4), note = b0 & 15 (1..13): values, notes 1..13 left to right:

| octave | P for notes 1..13 |
|---|---|
| 0 | 0xD58, 0xCA0, 0xBE8, 0xB38, 0xA98, 0x938, 0x970, 0x8E8, 0x868, 0x7F0, 0x780, 0x710, 0x6B0 |
| 1 | 0x6AC, 0x650, 0x5F4, 0x59C, 0x54C, 0x500, 0x4B8, 0x474, 0x434, 0x3F8, 0x3C0, 0x388, 0x358 |
| 2 | 0x356, 0x328, 0x2FA, 0x2CE, 0x2A6, 0x280, 0x25C, 0x23A, 0x21A, 0x1FC, 0x1E0, 0x1C4, 0x1AC |
| 3 | 0x1AB, 0x194, 0x17D, 0x167, 0x153, 0x140, 0x12E, 0x11D, 0x10D, 0x0FE, 0x0F0, 0x0E2, 0x0D6 |
| 4 | 0x0D6, 0x0CA, 0x0BF, 0x0B4, 0x0AA, 0x0A0, 0x097, 0x08F, 0x087, 0x07F, 0x078, 0x071, 0x06B |

Envelope value map for a nibble n (16 values): 0, 1, 2, 3, 4, 6, 9, 13, 18, 24, 31, 39, 48, 50, 61, 72.

Writes, in this order (all on voice v; DMACON bit is 1 << v):

1. DMACON = 1<<v (clear: this voice's DMA off).
2. A wait for 9 raster-line changes (section 7.4).
3. Parking: AUDxLC = the player's silent sample address (any zero data, 10 bytes or more; 0x0068ACB0 in
   the reference logs), AUDxLEN = 5, AUDxVOL = 0, AUDxPER = 0x00CA.
4. The note: AUDxLC, AUDxLEN, AUDxPER, with the values below.
5. The voice's bit is added to the tick's DMA mask (4.5). No AUDxVOL write is made at note start.

Cases by b1 (b2 = i is the sample slot):

| b1 | state after | AUDxLC | AUDxLEN (16 bit) | other effects |
|---|---|---|---|---|
| 0 | idle; volume = 0x40; sample index = none | A2 | ((T1 - 0x68) >> 1) & 0xFFFF (T1 - 0x68 as 32-bit signed, arithmetic shift) | attack/release values and the timer are left unchanged |
| 0x01..0x0F (high nibble 0) | release; volume 0x40 | A2 | ((T3 + T4) >> 1) & 0xFFFF | attack value = map[0] = 0; release value = map[b1 & 15]; timer = release value |
| 0x10..0xFF | attack; volume 0 | A2 | ((T3 + T4) >> 1) & 0xFFFF | attack value = map[b1 >> 4]; release value = map[b1 & 15]; timer = attack value |

In all three cases AUDxPER = P, the voice's current period becomes P, and for b1 != 0 the sample index
is i. (The stored command byte is NOT cleared by a note: a slide command stays active until another
command row on that voice or a song end/loop clears it.)

### 4.4 Command rows

b0, b1 and b2 are stored for the voice: command byte = b0, argument = b1 x 256 + b2. Nothing is written
now. The command stays in force every tick (section 5.2) until the next command row on this voice or a
song end (4.2, 3.2). Only 0x50, 0x51 and 0x52 act; other bytes 0x53..0xFE are stored and inert.

### 4.5 DMA enable

After the rows: if at least one note started in this tick, DMACON is written with 0x8200 OR the mask
(bit v for each voice that started a note); then a 9-line raster wait follows (7.4). Nothing is written
when no note started.

## 5. Voice update (every tick), voices 0..3 in order

For each voice, in this order:

### 5.1 Sample address refresh

* If the voice is idle (state idle): AUDxLC = the silent sample address, AUDxLEN = 5.
* Otherwise, if the running note's sample has a non-zero loop length T4: AUDxLC = A2 + T3 (loop start),
  AUDxLEN = (T4 >> 1) & 0xFFFF. (Paula latches these for the next restart of the sample; they are written
  every tick.)
* Otherwise nothing is written.

(Sample index "none" with a non-idle state cannot occur: a note without envelope is idle.)

### 5.2 Command

* 0x50: period += argument (16-bit wrap); AUDxPER = new period (written every tick).
* 0x51: period -= argument; AUDxPER = new period.
* 0x52: volume = attack value of the voice (as set by the last enveloped note), and the envelope step
  (5.3) is skipped this tick. No AUDxPER write.
* Anything else: no effect.

The effect runs for idle voices too (the period just changes, and AUDxPER is written).

### 5.3 Envelope (skipped when the command was 0x52)

Timer, volume and period arithmetic is 16 bit (wrapping).

* Idle: nothing.
* Attack: if the timer is non-zero, decrement it. Otherwise if the volume is not 0x40, add 8 and set the
  timer to attack value - 1. Otherwise (volume = 0x40) the voice enters release: timer = release value
  - 1, and then (in the same tick) the release rule below is applied once.
* Release: if the timer is non-zero, decrement it. Otherwise if the volume is not zero, subtract 8 and
  set the timer to release value - 1. Otherwise (volume = 0) the voice becomes idle.

Because of the minus 1, a value of 0 gives a timer of 0xFFFF (a wait of about 21 minutes), a value of 1
steps every tick. Volumes below 8 or above 0x40 cannot arise from steps; the attack-value volume of
command 0x52 can be any map value (up to 72) and then steps continue from it: the attack test is on
"equal to 0x40", so a volume above 0x40 keeps rising by 8 without bound while the attack lasts.

### 5.4 Volume write

AUDxVOL = volume & 0x7F (the host's default balance and volume are 0x40, which leave it unchanged). Written
every tick for every voice, including idle voices (value 0 or the 0x40 of a b1 = 0 note).

### 5.5 Order of writes in a voice's update

[AUDxLC, AUDxLEN] (if any), [AUDxPER] (if command 0x50/0x51), AUDxVOL. Voices 0..3 follow each other.

## 6. Timing ledger (cycles from the tick start)

### 6.1 Light tick

The update block (6.3) starts at cycle 397.

### 6.2 Full tick

**Row slots.** Slot v (v = 0..3) begins at b_v. b_0 = 375, b_(v+1) = e_v + 73 where e_v is the end of slot v.
If the song ended in slot v (4.2), slots after v do not run and e_v is the last slot end.

Slot lengths e_v - b_v by row kind (the wait, if any, is separate):

| row kind | length (all voices) |
|---|---|
| command row | 775 |
| note with period 0 | 670 |
| note with b2 >= 0x20 | 953 |
| song end by row byte 0xFF | report at b_v + 620 (see 6.4) |
| song end by order entry 0xFFFF | report at b_v + 345 (see 6.4) |
| accepted note | see below |

Accepted note, per voice (v = 0..3), cycles from b_v: DMACON(1<<v) write at 1084; the wait starts at
1102 and ends at X (cycles from the tick start, see 7.4). After X: parking writes at X+118 (AUDxLC long),
X+134 (AUDxLEN), X+150 (AUDxVOL), X+166 (AUDxPER). Then, by envelope kind (0 = b1 zero, T = high nibble zero,
S = other):

| kind | AUDxLC at X+ | AUDxLEN at X+ (v0, v1, v2, v3) | AUDxPER at X+ (v0..v3) | slot end e_v = X+ (v0..v3) |
|---|---|---|---|---|
| 0 | 514 | 693, 724, 755, 763 | 802, 864, 926, 942 | 1105, 1198, 1291, 1315 |
| T | 724 | 915, 946, 977, 985 | 1024, 1086, 1148, 1164 | 1297, 1390, 1483, 1507 |
| S | 710 | 901, 932, 963, 971 | 1010, 1072, 1134, 1150 | 1283, 1376, 1469, 1493 |

**After the slots.** Let e be the end of the last slot that ran. If any note started: DMACON (4.5) is written
at e + 118, the wait starts at e + 136 and ends at X, and the update block starts at X + 100. If no
note started the update block starts at e + 150.

(The tick routine's tail after the last voice update, 211 to 395 cycles, writes nothing and is not needed.)

### 6.3 Update block

Voice slots start at u_0 = block start, u_(v+1) = u_v + D_v + 45. Offsets are from u_v.

Reload class R (what 5.1 does): Z (idle voice): AUDxLC at 278, AUDxLEN at 300; L (loop): AUDxLC at 450,
AUDxLEN at 472; N (no loop, nothing written). Extra cost RC added to everything after the reload stage:
Z 67, L 239, N 0. (A third class, "idle by missing sample index", with
AUDxLC at 329 and RC 118, only arises for states that cannot occur.)

AUDxPER (5.2) is written at 792 + RC for command 0x50 and at 811 + RC for 0x51.

AUDxVOL is written at VO = 1120 + A(v) + RC + EF + EV, with
A(v) = 0, 23, 46, 36 for v = 0..3; EF (command effect, v = 0..3): none 0,0,0,0; 0x50: 353, 384, 415, 423;
0x51: 372, 403, 434, 442; EV by envelope case (decided by the state before this tick's step):

| case | EV |
|---|---|
| attack, timer non-zero | 0 |
| attack, timer zero, volume != 0x40 | 83 |
| attack, timer zero, volume = 0x40, release value = 1 | 237 |
| attack, timer zero, volume = 0x40, release value != 1 | 154 |
| release, timer non-zero | 19 |
| release, timer zero, volume != 0 | 102 |
| release, timer zero, volume = 0 (becomes idle) | 76 |
| idle | -32 |

With command 0x52 the whole envelope part is replaced: VO = 1134 + A(v) + RC.

The slot length is D_v = VO + TAIL(v), TAIL = 205, 236, 267, 275 for v = 0..3.

### 6.4 Song-end report

When the song ends inside a row slot (4.2) or after the position test (3.2) the host call costs time.
Let `ct` be the cycle count at the instant of the call (given above as an offset from the slot start: 620
or 345; for the position test: last voice slot start u_3 + D_3 + 301). The host drops a half cycle and adds
16: the count continues from `ct - (ct mod 2) + 16`. For a row slot the slot then ends 156 cycles after
that: `e_v = ct - (ct mod 2) + 16 + 156`. For the position test nothing is written afterwards. The odd/even
parity of `ct` is the parity of the cycle count itself (counted from the tick start), so keep exact
cycle counts and halve only for stamps.

## 7. Wait routine and beam

### 7.1 Clock and beam

The raster line at a clock t (cc) is floor(t / 227) mod 313; the wait routine looks at its low 8 bits.
A read is considered to take place at the end of the instruction that makes it, at clock
`tick_start + floor(c/2)` for the cycle count c reached.

### 7.4 Nine-line wait

The routine, entered at cycle count c: costs 28 cycles at entry. Then nine rounds, each: 16 cycles, take
a reference reading of the line; then poll: 16 cycles, read the line, 9 cycles; repeat the poll while the
reading equals the reference; when it differs: 10 cycles and the next round begins. After the ninth round,
44 cycles. (The sample at each reading is taken at the cycle count after the 16-cycle read cost.) The
wait therefore ends within one poll period (25 cycles) after the ninth line change, and its length varies
with the beam phase. The cycle count X after the wait (used above) includes the 44.

## 8. Start-up

The host runs, before the first tick, the player's detection and its initialisation. Let (all from the
module):

* W = number of order-list words before the first 0xFFFF word; M = number of those words that are >= the
  largest of the preceding words (the running maximum starts at 0); Z = number of zero 32-bit words
  skipped in 1.1 step 4. Cost A = 315 + 74 W + 8 M + 21 Z cycles.
* Over the 32 slots in order, with a running value d (start 0): ne = slots whose table-0 high byte is
  non-zero; for other slots, if d >= table-0 entry then nn counts it; else nm counts it and
  d = table-0 + table-1 (32 bit, signed comparison). Cost B = 6 + 53 ne + 110 nm + 84 nn cycles.

First tick: **E0 = 84319 + floor(A/2) + floor(B/2)** cc.

Eight AUDxVOL = 0 writes happen at start-up, two volume-setting calls of the host: the first at
E0 - 84191 and the second at E0 - 83527 cc, each writing voices 0..3 at +floor(c/2) cc for c = 440, 705,
993, 1271 (registers AUDxVOL of voice 0, 1, 2, 3 in that order). They are the first eight lines of every
log.

## 9. Relation to Speedy System

Shared in spirit only: same author, 4 voices, 50 Hz timer, speed-counter ticking (S+1 ticks per row),
attack/release envelope in steps of 8, slide effects 0x50/0x51 on the period with a 16-bit argument,
9-line raster wait before the DMA-on write. Everything concrete differs: file layout (no text magic, no
position tables of 4 x 256 pointers, no run codes, 48-byte tracks, 32 sample slots with five tables), the
period table (65 values here), the envelope map table (values up to 72 here, 16 and 24 there), the note
start sequence (park writes and a wait per note here), sample handling (loop addresses rewritten every
tick here), song end (order entry 0xFFFF or row byte 0xFF; the counter is not reset here), timing (every
cost differs).
